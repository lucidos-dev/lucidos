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
    description: "Which notifications to return: 'unread' (default) or 'all'.",
};
const LIMIT_ARG: Arg = Arg {
    name: "limit",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Maximum number of notifications to return (1-50, default 20).",
};
const NOTIFICATION_ID_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "UUID of the notification to mark read (from the 'list' action).",
};

const NOTIFICATIONS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "List inbox notifications (unread by default, or all). Returns id/title/message/read/created_at.",
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
        summary: "Mark every unread notification read (clears the inbox badge).",
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
    description: "Device id to read device-scoped overrides for; omit for the global view.",
};
const PREF_KEY_ARG: Arg = Arg {
    name: "key",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "Preference key (e.g. 'theme', 'language', 'timezone', 'chat_model').",
};
const PREF_VALUE_ARG: Arg = Arg {
    name: "value",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "New value as a string ('true'/'false' for booleans, '125' for numbers, an allowed enum value).",
};
const PREF_SET_DEVICE_ID_ARG: Arg = Arg {
    name: "device_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Device id for a per-device key; omit for global keys.",
};

const PREFERENCES_OPS: &[Operation] = &[
    Operation {
        action: "get",
        summary: "List the settable preferences with each one's current value, allowed values, default, and scope (global vs per-device).",
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
        summary: "Change a single preference (theme, language, timezone, chat_model, …). Call 'get' first if unsure of the key or its allowed values.",
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
              "key": {"type":"string","description":"The preference key, e.g. 'theme', 'language', 'timezone', 'chat_model'. Use the 'get' action to see the full list of settable keys."},
              "value": {"type":"string","description":"The new value, as a string. Booleans are 'true'/'false'; numbers like ui-scale are '125'; enums must match an allowed value (see the 'get' action)."}
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
    tool_summary: "Read and change user preferences (Settings). 'get' lists every \
        settable key with its current value, allowed values, default, and scope \
        (global vs per-device); 'set' changes one key. Call 'get' before 'set' when \
        unsure of a key or its allowed values. Device-scoped keys (theme, font, \
        ui-scale, push) auto-apply to the calling device. This does NOT set secrets \
        (use request_credential), add chat models (use manage_models), or change \
        command-safety settings.",
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
    description: "JSON run config to change: { \"type\": \"intent\", \"intent\": \"…\" } or { \"type\": \"script\", \"path\": \"…\" }.",
};
const TRIGGER_CRON_EXPRESSIONS_ARG: Arg = Arg {
    name: "cron_expressions",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description:
        "JSON array of 6-field cron strings in the user's local time, e.g. [\"0 0 8 * * *\"].",
};
const TRIGGER_ON_ARG: Arg = Arg {
    name: "on",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description:
        "JSON array of event subscriptions, e.g. [{\"event_type\":\"X\",\"condition\":{...}}].",
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
    description:
        "Owning app directory name (e.g. 'trigger-workflow'); deep-links notifications to that app.",
};
const TRIGGER_GO_TO_REVIEW_ARG: Arg = Arg {
    name: "go_to_review",
    ty: ArgType::Bool,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "When true, threads spawned by this trigger surface in REVIEW on completion instead of ARCHIVE.",
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
    description: "JSON array of irreversible side-effect categories this trigger may perform unattended (e.g. [\"email\"]).",
};
const TRIGGER_SLUG_ARG: Arg = Arg {
    name: "slug",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Stable kebab-case slug (directory segment for per-trigger knowhow); derived from name when omitted.",
};

// The grouped LLM tool keeps the existing flat-tool shapes (shorthand cron/on,
// run object) so execute_scheduler_tool reads the args unchanged. `cron`/`on`
// allow null so the unioned property serves both create and update (clearing).
const TRIGGER_CREATE_LLM_SCHEMA: &str = r#"{
  "name": {"type":"string","description":"A short, descriptive name for the trigger."},
  "run": {"type":"object","description":"What to execute. Either { type: 'intent', intent: '…' } for an LLM intent (one sentence in the user's voice — keep procedure out of the intent; the trigger loads knowhow itself at fire time), or { type: 'script', path: 'name/run.py' } for a script."},
  "cron": {"description":"Cron schedule(s), 6 fields in the USER'S LOCAL TIME (second minute hour day-of-month month day-of-week). A single string, an array of strings, or null. Example: '0 0 8 * * *' for 8am daily.","oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"},"minItems":1},{"type":"null"}]},
  "on": {"description":"Event subscriptions. Each entry is { event_type: 'X', condition?: {…} } (condition operators $eq/$ne/$lt/$lte/$gt/$gte/$in). A bare string, an array of strings/objects, or null.","anyOf":[{"type":"null"},{"type":"string"},{"type":"array","items":{"anyOf":[{"type":"string"},{"type":"object","properties":{"event_type":{"type":"string"},"condition":{"type":"object"}},"required":["event_type"]}]}}]},
  "app_id": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Owning app directory name (e.g. 'trigger-workflow'); notifications deep-link to that app. Omit/null for standalone triggers."},
  "go_to_review": {"type":"boolean","description":"When true, threads spawned by this trigger surface in REVIEW on completion instead of ARCHIVE. Default false."},
  "group_id": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Trigger-group id (from list_trigger_groups). Organizational only. Omit/null for ungrouped."}
}"#;
const TRIGGER_UPDATE_LLM_SCHEMA: &str = r#"{
  "trigger_id": {"type":"string","description":"UUID of the trigger to update/delete/pause/resume."},
  "paused": {"type":"boolean","description":"Pause (true) or resume (false) as part of a multi-field update; prefer the pause/resume actions for that alone."}
}"#;

const TRIGGERS_OPS: &[Operation] = &[
    Operation {
        action: "create",
        summary: "Create a NEW trigger (schedule-based via cron, event-based via on, or both). list/update existing workflows instead of recreating. Set timezone first.",
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
        summary: "List all triggers with their names, schedules, event subscriptions, and what each runs.",
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
        summary: "Update an existing trigger's name/schedule/subscriptions/run config. Prefer over delete+create (keeps run history). Send the full replacement 'on' array.",
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
        summary: "Delete a trigger by id (orphans its run history — prefer update for tweaks).",
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
        summary: "Pause a trigger so it stops firing on its schedule and stops matching events (config preserved).",
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
        summary: "Resume a previously paused trigger so it fires on its schedule and matches events again.",
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
];

const TRIGGERS_DOMAIN: Domain = Domain {
    name: "triggers",
    tool_name: "triggers",
    tool_summary: "Create and manage triggers — scheduled (cron) and/or event-driven \
        automations. 'create' a new trigger, 'list' existing ones, 'update' a trigger \
        in place (prefer over delete+create — keeps run history), 'delete', and \
        'pause'/'resume'. Cron times are in the user's local timezone (set it first). \
        To organize triggers into panel folders, use the trigger_groups tool.",
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
        summary: "List trigger groups (id, name, order, member_count). Pure organizational folders.",
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
        summary: "Create a trigger group (a named folder). Names are unique (case-insensitive).",
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
        summary: "Rename a trigger group. Fails if another group already uses the new name (case-insensitive).",
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
        summary: "Atomic batch reorder of trigger groups — pass an array of { id, order } entries.",
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
        summary: "Delete a trigger group. Refuses (with member ids) when the group still has triggers — move them first.",
        method: Method::Delete,
        path: "/trigger-groups",
        args: &[TG_ID_QUERY_ARG],
        cli_name: "delete",
        sdk_name: "delete",
        mutating: true,
        llm_alias: Some("delete_trigger_group"),
        llm_schema: Some(r#"{"group_id":{"type":"string","description":"UUID of the group to delete."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const TRIGGER_GROUPS_DOMAIN: Domain = Domain {
    name: "trigger_groups",
    tool_name: "trigger_groups",
    tool_summary: "Manage trigger groups — user-visible folders that organize triggers \
        in the panel. Pure organizational label: groups don't fire or schedule \
        anything. 'list' groups, 'create' / 'rename' / 'delete' a group, or 'reorder' \
        them. Assign a trigger to a group via the triggers tool's group_id field.",
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
        summary: "List all apps in the workspace (id, name, description, icon).",
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
        summary: "Get one app's metadata by id.",
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
        summary: "Update an app's name/description.",
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
        summary: "Delete an app by id (plugin-installed apps must be removed via the plugin).",
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
    tool_summary: "Manage apps — 'list' all apps, 'get' one by id, 'update' an \
        app's name/description, or 'delete' an app. (Creating an app is the \
        separate create_app tool; editing app source is done in the app's \
        coding-agent worktree.)",
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
  "event_type": {"type":"string","description":"Event type in PascalCase past tense (e.g., GoogleDocEdited, DataImported)."},
  "payload": {"type":"object","description":"Event payload — REQUIRED. Include enough context to understand what happened.","properties":{"summary":{"type":"string","description":"Human-readable description of what happened"}},"required":["summary"]}
}"#;
const EVENTS_QUERY_LLM_SCHEMA: &str = r#"{
  "event_type": {"type":"string","description":"Filter by event type (e.g., DataImported). Omit to query all — but prefer a specific filter on busy workspaces."},
  "since": {"type":"string","description":"Only return events after this ISO 8601 / RFC 3339 timestamp."},
  "until": {"type":"string","description":"Only return events before this ISO 8601 / RFC 3339 timestamp."},
  "limit": {"type":"integer","description":"Max events (1-200, default 50). Raise only for full enumeration of a small type."},
  "byte_limit": {"type":"integer","description":"Per-call byte budget for the compact-JSON response (1024-524288, default 131072 = 128 KB). On truncation follow the response hint — narrow the query before bumping this."}
}"#;
const EVENTS_COUNT_LLM_SCHEMA: &str = r#"{
  "event_type": {"type":"string","description":"Filter by event type. Omit for a per-type breakdown across all event types."},
  "since": {"type":"string","description":"Only count events after this ISO 8601 / RFC 3339 timestamp."},
  "until": {"type":"string","description":"Only count events before this ISO 8601 / RFC 3339 timestamp."}
}"#;

const EVENTS_OPS: &[Operation] = &[
    Operation {
        action: "emit",
        summary: "Emit a domain event (immutable, past-tense fact). The payload must include a 'summary'. (requires: event_type, payload)",
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
        summary: "Query events newest-first, wrapped as {events, total_matching, returned, byte_size, truncated, hint?}. Call 'count' first to size a busy sweep; treat 3 calls/turn as a soft ceiling.",
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
        summary: "Count events by type/time without materialising payloads. With event_type → {count, byte_total}; without → a per-type breakdown sorted by count desc. Call BEFORE 'query' on busy windows.",
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
    tool_summary: "Work with the workspace's domain-event store. 'emit' records an immutable \
        past-tense fact (payload must include a 'summary'); 'query' reads events newest-first \
        (honours a 128 KB byte budget — narrow on truncation); 'count' sizes a sweep by type/time \
        without materialising payloads. On a busy workspace, 'count' first, then 'query' the \
        narrowest type(s) — do NOT enumerate everything into context.",
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
        summary: "List pending + recently-applied changes (coding-agent branches awaiting Apply). Returns { pending, applied, total_pending }; read .pending[].id before 'apply'. Read-only.",
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
        summary: "Apply a pending change — merge the coding-agent branch into main, exactly as the Apply button does. ONLY when the user asked. Returns the typed apply result (status/SHAs/restart_required). (requires: change_id)",
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
    tool_summary: "Inspect and apply *changes*: coding-agent-proposed branches awaiting the Apply \
        button. 'list' returns pending + recently-applied changes (find a change's id here before \
        applying); 'apply' merges one into the workspace's main exactly as the Apply button does \
        (Lucidos-source applies run /harden and may need a restart; app applies ff-merge). Only \
        'apply' when the user asked to.",
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
    description: "Model id — the string sent in API requests (e.g. 'z-ai/glm-5.2', 'claude-opus-4-8@default'). Required for add/enable/disable/remove.",
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
    description: "Human-friendly display name for 'add' (defaults to the id).",
};
const MODEL_PROVIDER_ARG: Arg = Arg {
    name: "provider",
    ty: ArgType::Str,
    enum_values: MODEL_PROVIDER_ENUM,
    required: true,
    loc: ArgIn::Body,
    description: "Backend that serves the model. Required for 'add'.",
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
    description: "Optional display order (lower sorts first; user models default to 1000).",
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
    description: "Context window in tokens (e.g. 1048576). Set it to the window the model actually serves. Omitting it falls back to guessing from the model id (claude-* → 200k unless the id ends in [1m], gpt-5* → 400k, anything else → 200k) — that guess has no rule for OpenRouter / Gemini / local ids, so they are treated as 200k however large they really are. Guesses err low on purpose: too low only trims context early, too high makes the provider reject the request.",
};

const MODELS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "List all models in the registry (enabled + disabled, builtin + user).",
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
        summary: "Enable an existing model (show it in the picker).",
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
        summary:
            "Disable an existing model (hide it from the picker; builtins disable, never delete).",
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
        summary: "Edit a model's label/provider/sort_order/enabled (CLI generic PUT).",
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
        summary: "Delete a user-added model (builtins can't be removed — disable instead).",
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
    tool_summary: "Manage the chat-model registry that powers the Lucidos Agent's model picker. \
        Add a model the user wants available, enable/disable an existing one, or remove a \
        user-added model. To switch the ACTIVE model instead, use set_preference(key='chat_model'). \
        Builtin models can be disabled but not removed.",
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
// misc::get_manage_repositories_tool). LLM-only: add/remove run in-process via
// RepositoryStore (no add/remove HTTP route), so cli/sdk = false — pure
// drift-safety, no new surface. execute_tool keeps routing manage_repositories →
// the unchanged execute_manage_repositories handler. See engine/tools/mod.rs.
// ---------------------------------------------------------------------------

const REPO_NAME_ARG: Arg = Arg {
    name: "name",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description:
        "Repository display name (required for 'add'; used to find the repo for 'remove').",
};
const REPO_PATH_ARG: Arg = Arg {
    name: "path",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Absolute path to the git repository on disk (required for 'add'). Supports ~/.",
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
        summary: "Unregister a repository by name.",
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
    tool_summary: "Manage registered external git repositories for coding-agent sessions. Users \
        can register local repos so Claude Code or Codex can work on them.",
    llm: true,
    // add/remove are in-process (RepositoryStore) with no HTTP route, so no
    // generated CLI/SDK is possible — declared N/A. Pure schema-SSOT migration.
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
    description: "Variable name: uppercase letters, digits, underscores; not starting with a digit (e.g. GITHUB_TOKEN). Engine-owned names (CRED_*, OAUTH_*, PG*, PATH, internal LUCIDOS_*) are rejected.",
};
const ENV_VALUE_BODY_ARG: Arg = Arg {
    name: "value",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Variable value (plaintext, non-secret). For secrets use a credential instead.",
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
        summary: "List all user environment variables (name + value). These are injected into every subprocess Lucidos spawns.",
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
        summary: "Create or replace a non-secret environment variable. Takes effect on the next subprocess — no restart.",
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
        summary: "Remove an environment variable by name.",
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
    tool_summary: "Manage non-secret environment variables injected into every subprocess Lucidos \
        spawns (run_bash, run_python, scheduled scripts, coding agents). 'list' shows all \
        name+value pairs, 'set' creates or replaces one (takes effect on the next subprocess — no \
        restart), 'delete' removes one by name. These are NOT secret (they appear in logs/events) — \
        for API keys, tokens, or passwords use request_credential instead.",
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

const THREADS_LIST_LLM_SCHEMA: &str = r#"{
  "active": {"type":"boolean","description":"When true, restrict to threads where the agentic loop is mid-flow (running or waiting_for_user_answer); when false, invert. Omit for no filter. Note: 'waiting' is NOT active (the coding agent stopped and proposed changes)."},
  "source": {"type":"string","description":"Filter by source. Comma-separated list of 'chat', 'trigger', 'coding-agent' (legacy 'claude_code' also accepted). Omit for all."},
  "limit": {"type":"integer","description":"Maximum number of threads to return (1-1000, default 100)."}
}"#;
const THREADS_COUNT_LLM_SCHEMA: &str = r#"{
  "active": {"type":"boolean","description":"When true, count only threads mid-flow (running or waiting_for_user_answer); when false, the inverse. Omit for total count."},
  "source": {"type":"string","description":"Filter by source. Comma-separated list of 'chat', 'trigger', 'coding-agent'. Omit for all."}
}"#;

const THREADS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "List thread summaries newest-first (the projection rows). Cheaper than query_events for 'what threads exist / their status / age'. Each row carries thread_id, title, channel, status, last_activity, parent_thread_id, trigger_id, …",
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
        summary: "Count thread summaries matching the same filters as 'list'. Returns { count: N } — the cheap 'is anything still running?' check.",
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
    tool_summary: "Introspect threads. 'list' returns thread summaries newest-first (thread_id, \
        title, channel, status, last_activity, parent/trigger ids); 'count' returns just the \
        matching count. Both take the same optional filters (active, source, limit) — far cheaper \
        than query_events for 'what threads exist / their status'. To START a thread, use the \
        separate run_thread / run_coding_agent tools.",
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
    description: "Comma-separated importance levels to include: low,medium,high,critical.",
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
        summary: "Search for and correct wrong memories by keyword + semantic match to the wrong claim. (requires: search_query, wrong_fact)",
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
              "wrong_fact": {"type":"string","description":"The specific wrong claim to delete (e.g., 'User works at Acme Corp'). Only memories semantically similar to this are deleted."},
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
        summary: "Delete (and optionally replace) ONE memory by its id — the precise path when the [id: <uuid>] is visible in the [Long-term Memory] block. (requires: id)",
        method: Method::Post,
        path: "/memory/correct",
        args: &[],
        cli_name: "correct-by-id",
        sdk_name: "correctById",
        mutating: true,
        llm_alias: Some("correct_memory_by_id"),
        llm_schema: Some(
            r#"{
              "id": {"type":"string","description":"The memory entry's id (a UUID), copied verbatim from the [id: <uuid>] at the end of its bullet in the [Long-term Memory] block."},
              "correction": {"type":"string","description":"Optional corrected fact to store after deleting this entry. Omit to just delete."}
            }"#,
        ),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "stats",
        summary: "Memory index stats (entry counts, sources). Read-only.",
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
        summary: "Paginated long-term-memory entries with their importance and source. Read-only.",
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
        summary: "Inspect one memory's source (the originating event or artifact) plus the entries derived from it. Read-only.",
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
    tool_summary: "Correct long-term memory. 'correct' searches by keyword and deletes only the \
        entries that semantically match a wrong claim (optionally storing a correction); \
        'correct_by_id' removes exactly one entry by the [id: <uuid>] shown in the [Long-term \
        Memory] block. Use 'correct_by_id' when the id is visible, 'correct' otherwise. (Reading \
        memory is the `lucidos memory` CLI; the agent gets memory injected into its context.)",
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
  "max_concurrent_total": {"type":"integer","minimum":0,"description":"Maximum concurrently running threads across all kinds — background spawns AND user-initiated work."},
  "max_concurrent_event_trigger": {"type":"integer","minimum":0,"description":"Maximum concurrently running event-trigger fires."},
  "max_concurrent_cron": {"type":"integer","minimum":0,"description":"Maximum concurrently running cron-trigger fires."},
  "max_concurrent_sub_thread": {"type":"integer","minimum":0,"description":"Maximum concurrently running agent-spawned sub-thread chats."},
  "max_concurrent_coding_agent": {"type":"integer","minimum":0,"description":"Maximum concurrently running agent-spawned coding-agent threads."},
  "max_concurrent_per_trigger": {"type":"integer","minimum":0,"description":"Maximum concurrent runs for one trigger. 1 preserves strict per-trigger FIFO."},
  "max_queued_per_trigger": {"type":"integer","minimum":1,"description":"Maximum queued backlog for one trigger before overflow handling applies."},
  "reserved_background": {"type":"integer","minimum":0,"description":"Slots background work can always reclaim ahead of user-initiated work. 0 = pure user priority."},
  "overflow": {"type":"string","enum":["drop-oldest","pause-trigger"],"description":"Overflow behavior when one trigger reaches max_queued_per_trigger."}
}"#;

const THREAD_QUEUE_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "List the live Thread Queue + active capacity policy ({ entries, policy }), including user-initiated occupants. Read-only — call before changing capacity so relative requests are computed from the live policy.",
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
        summary: "Update the capacity policy. Only provided fields change (merged with the live policy); emits the persisted CapacityPolicyChanged. Caps may be 0 to hold admission; max_queued_per_trigger ≥ 1.",
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
    tool_summary: "Inspect and tune the Thread Queue — the shared admission-control pool for \
        background spawns AND user-initiated work. 'list' shows live entries + the active capacity \
        policy (call it before relative changes like 'double capacity'); 'update_policy' changes \
        caps in place (only provided fields change). Concurrency caps may be 0 to hold admission; \
        keep max_concurrent_per_trigger at 1 unless one trigger should run fires concurrently.",
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
  "id": {"type":"string","description":"Unique identifier for this server (e.g., 'blender-mcp', 'roblox-studio'). Use lowercase with hyphens."},
  "name": {"type":"string","description":"Human-readable name (e.g., 'Blender MCP', 'Roblox Studio MCP')"},
  "command": {"type":"string","description":"Command to run the MCP server (e.g., 'npx', 'uvx', 'node')"},
  "args": {"type":"array","items":{"type":"string"},"description":"Arguments for the command (e.g., ['blender-mcp'] for 'uvx blender-mcp')"},
  "env": {"type":"object","additionalProperties":{"type":"string"},"description":"Optional environment variables for the server process"}
}"#;

const MCP_OPS: &[Operation] = &[
    Operation {
        action: "setup",
        summary: "Register and connect a new MCP server (spawns the process and discovers its tools). web_search first to find the right package + command. (requires: id, name, command, args)",
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
    tool_summary: "Manage MCP (Model Context Protocol) servers: 'setup' (register + connect a new \
        server, spawning the process and discovering its tools), 'list' configured servers with \
        their status, and 'start'/'stop'/'remove' an existing one by id. Use web_search first to \
        find the right package + install command for the server the user wants.",
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
        summary: "Stage a plugin install for the user to confirm in a panel (GitHub tree URL, git URL, or a .lucidos-plugin path). Do NOT respond about success after calling — the panel resolves it. (requires: source)",
        method: Method::Post,
        path: "/plugins/install-request",
        args: &[],
        cli_name: "install",
        sdk_name: "install",
        mutating: true,
        llm_alias: Some("install_plugin"),
        llm_schema: Some(r#"{"source":{"type":"string","description":"GitHub tree URL (e.g. 'https://github.com/lucidos-dev/plugins/tree/main/browser-learning'), a plain git URL, or an absolute path to a .lucidos-plugin file."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "register_marketplace",
        summary: "Register or rename a plugin marketplace (a git repo / GitHub tree URL scanned for plugin manifests) that the Plugins panel browses. (requires: source)",
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
        summary: "Check installed plugins for newer versions at their source URL. Omit id to survey all installed plugins.",
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
        summary: "Apply the update for one installed plugin (re-fetches the manifest, re-installs if newer). (requires: id)",
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
        summary: "Stage a plugin uninstall for the user to confirm in a panel (resolves id against plugin id, manifest name, or app folder). Do NOT respond about success after calling. (requires: id)",
        method: Method::Post,
        path: "/plugins/uninstall-request",
        args: &[],
        cli_name: "uninstall",
        sdk_name: "uninstall",
        mutating: true,
        llm_alias: Some("uninstall_plugin"),
        llm_schema: Some(r#"{"id":{"type":"string","description":"Plugin id, manifest name, or app folder installed by the plugin (e.g. 'browser-learning'). Case- and dash/underscore/whitespace-insensitive."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const PLUGINS_DOMAIN: Domain = Domain {
    name: "plugins",
    tool_name: "plugins",
    tool_summary: "Manage Lucidos plugins — coherent bundles of workspace content (apps, knowhow, \
        triggers, scripts) another author shipped. 'install' stages a confirm panel for a plugin \
        source, 'uninstall' stages a removal panel, 'register_marketplace' adds a marketplace source the Plugins panel browses, \
        and 'check_updates'/'update' survey and apply newer versions. install/uninstall resolve in a \
        panel — after calling them, do NOT claim success; the next user message reports the outcome.",
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
        tool_summary: "Read and clear the notification inbox. Use 'list' to see what \
            notifications have been sent (task errors, agent nudges, etc.), 'mark_read' \
            to clear one by id, and 'mark_all_read' to clear the whole unread inbox. To \
            SEND a notification, use the separate send_notification tool.",
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
        description.push_str(&format!("\n• {} — {}", op.action, op.summary));
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
            vec!["create", "list", "update", "delete", "pause", "resume"]
        );
        // pause/resume are LLM-only (no dedicated HTTP route).
        let pause = triggers
            .operations
            .iter()
            .find(|o| o.action == "pause")
            .unwrap();
        assert!(pause.on_llm(triggers) && !pause.on_cli(triggers) && !pause.on_sdk(triggers));

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
