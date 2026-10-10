//! Defense against the chat model HTML-entity-escaping text it puts in a
//! tool-call argument, so a name the user reads as `Machine & Tooling Health`
//! is created, persisted and re-served as `Machine &amp; Tooling Health`.
//!
//! This is a **model-tolerance measure**, tracked in
//! `docs/temporary-measures.md` §2 under the `model-escapes-tool-arg-text`
//! investigation. It is a sibling of [`super::inline_question_repair`] and
//! [`super::inline_tool_call_repair`]: same boundary, same failure class (the
//! model's own serialization habits leaking into a channel that should carry
//! literal text), a different shape.
//!
//! ## Why the model, and not us
//!
//! Bisected 2026-08-09 between "provider stream bytes" and the `ToolCalled`
//! emit, because the write path was the obvious suspect:
//!
//! - **Nothing on the write path escapes.** The only entity-encoding sites in
//!   the tree build standalone HTML pages (`core/oauth.rs`, `api/base_path.rs`,
//!   the gateway proxy, the desktop shell); the frontend's escapes are all
//!   render-time. `llm/anthropic_wire.rs` concatenates `partial_json` deltas
//!   and hands the string to `serde_json::from_str` untouched, and the only
//!   transforms before the emit are `redact_postgres_secrets_in_json` (inert
//!   unless the string contains `postgres`) and `sanitize_for_jsonb` (strips
//!   NUL).
//! - **The model had nothing escaped to copy.** In the canonical turn, the two
//!   preceding tool results (a knowhow body and a trigger listing, ~57KB
//!   together) contained bare `&` and zero entities. The listing even showed
//!   the model a clean `Bump DMG link & publish on release`.
//! - **It is per-field, which no transport escape can be.** One observed
//!   `run_coding_agent` call carries `"title": "Nightly: build &amp; test"`
//!   beside `"prompt": "Build & test the engine…"` in the *same* arguments
//!   object.
//!
//! ## Why a per-tool allow-list of argument keys
//!
//! A blanket decode over every string argument would corrupt real data: the
//! model writes HTML through `write_file.content`, `create_app.html_content`
//! and `edit_file.new_string`, where `&amp;` is correct and must round-trip
//! byte-identical. An allow-list fails safe (a key we forgot leaves the bug in
//! place for that field); a deny-list fails destructive (a key we forgot
//! silently rewrites a user's file).
//!
//! The list is keyed by TOOL, and each entry is a full path from the argument
//! root, because the same word means different things in different schemas and
//! at different depths. `name` is a trigger group's label, the exact key
//! `proxy_request` looks up in `data/config/apis.json`, and somebody's stored
//! data at `edit_file`'s `new_value.name`. A tool with no row is declined
//! whole, which is how every third-party `mcp__<server>__<tool>` is handled:
//! not by a special case, but by never having claimed to know its schema.

use crate::llm::tool_names as tn;

/// The plain-text arguments of each tool, as full paths from the argument
/// root. A path earns a place here only for the tool whose schema makes it
/// prose the user reads verbatim, because the same word means different things
/// in different tools and at different depths: `name` is a trigger group's
/// label in `trigger_groups`, an exact lookup key into `data/config/apis.json`
/// in `proxy_request`, and somebody's stored data at
/// `edit_file`'s `new_value.name`.
///
/// Matching the whole path rather than the leaf key is what makes the negative
/// half self-evident: a payload the model composes for somewhere else is never
/// AT one of these paths, so `new_value.message`, `env.name` and
/// `on.condition.name` need no exclusion list. An array index is not a path
/// segment, so `questions.options.label` covers every option of every
/// question.
///
/// A tool absent from this table is declined whole, and that is the design
/// rather than an omission. It covers every `mcp__<server>__<tool>` (a schema
/// somebody else wrote, where none of this per-argument reasoning transfers),
/// and it covers the built-in tools whose look-alike arguments are
/// identifiers: `proxy_request.name` names an `apis.json` entry, and
/// `env_vars.name` is an environment variable name.
///
/// The back-compat flat aliases (`create_trigger_group`, `emit_event`, …) get
/// their own rows because the model still calls them, and the names come from
/// `tool_names` so a rename is a compile error rather than a silent gap.
///
/// Two absences are deliberate rather than accidental:
///
/// - **`prompt`**, on `run_thread` / `run_coding_agent`. It is a large
///   free-text body that routinely carries code, diffs and HTML, and the
///   evidence has it arriving clean beside a mangled `title` in the same call.
/// - **`todo_write`'s item text**, which is keyed `content`, the same key
///   `write_file` uses for a whole file body. Scoping is per tool, so this one
///   is expressible, but the todo list is the cheapest surface to lose
///   (per-thread, transient, never persisted as an artifact) and it is not
///   worth the chance of a future reader copying `content` onto a tool where
///   it means a file.
const PLAIN_TEXT_ARGS_BY_TOOL: &[(&str, &[&str])] = &[
    // A semantic commit message describing the edit, shown in the change view.
    // `edit_file`'s JSON-mode `new_value` is written verbatim into a `.json` or
    // `.slides` file, and a `message` inside THAT is at `new_value.message`, so
    // it is not this path and never rewritten.
    (tn::WRITE_FILE, &["message"]),
    (tn::EDIT_FILE, &["message"]),
    (tn::COPY_FILE, &["message"]),
    (tn::DELETE_FILE, &["message"]),
    // A trigger's display name, and its `run.intent` in the user's voice. The
    // `on[].condition` matcher next door is NOT a path here, so it is left as
    // the model wrote it: it is compared byte-for-byte against stored rows,
    // including ones written before this repair existed.
    (tn::TRIGGERS, &["name", "run.intent"]),
    (tn::CREATE_TRIGGER, &["name", "run.intent"]),
    (tn::UPDATE_TRIGGER, &["name", "run.intent"]),
    // A trigger group's label in the trigger panel. Both create and rename
    // carry it under `name`.
    (tn::TRIGGER_GROUPS, &["name"]),
    (tn::CREATE_TRIGGER_GROUP, &["name"]),
    (tn::RENAME_TRIGGER_GROUP, &["name"]),
    // The notification the user reads on a lock screen.
    (tn::SEND_NOTIFICATION, &["title", "message"]),
    // A child thread's title, which is also how the parent refers to it later.
    (tn::RUN_THREAD, &["title"]),
    (tn::RUN_CODING_AGENT, &["title"]),
    (tn::RUN_CLAUDE_LEGACY, &["title"]),
    // An instruction that lands in the child's conversation as a message.
    (tn::FOLLOW_UP_CHILD_THREAD, &["message"]),
    // The app's display name and one-line description. NOT `html_content`.
    (tn::CREATE_APP, &["name", "description"]),
    (tn::EXECUTE_INTENT, &["task"]),
    // Every string on a question card: the question, its chip label, each
    // option's button text and the explanation under it.
    (
        tn::ASK_USER_QUESTION,
        &[
            "questions.question",
            "questions.header",
            "questions.options.label",
            "questions.options.description",
        ],
    ),
    // The one-line `summary` an `emit_event` payload is required to carry, and
    // only that one: the rest of an arbitrary domain-event payload is stored
    // exactly as the model composed it, down to a deeper field that happens to
    // be called `summary` too.
    (tn::EVENTS, &["payload.summary"]),
    (tn::EMIT_EVENT, &["payload.summary"]),
    // The corrected fact stored in place of the wrong memories.
    (tn::MEMORY, &["correction"]),
    (tn::CORRECT_MEMORY, &["correction"]),
    (tn::CORRECT_MEMORY_BY_ID, &["correction"]),
    // Display names: a repository's, an MCP server's, a marketplace's. An MCP
    // server's `env` map is at `env.<VAR>`, never the bare `name`.
    (tn::MANAGE_REPOSITORIES, &["name"]),
    (tn::MCP, &["name"]),
    (tn::SETUP_MCP_SERVER, &["name"]),
    (tn::PLUGINS, &["name"]),
    (tn::REGISTER_PLUGIN_MARKETPLACE, &["name"]),
];

/// Decode XML predefined entities in `tool_name`'s plain-text arguments, in
/// place. Returns `true` if anything changed, so the caller can log the repair
/// and keep its frequency observable for the registry's removal condition.
///
/// A tool with no row in [`PLAIN_TEXT_ARGS_BY_TOOL`] is left completely alone.
pub(crate) fn repair_tool_arg_entities(tool_name: &str, args: &mut serde_json::Value) -> bool {
    let Some((_, paths)) = PLAIN_TEXT_ARGS_BY_TOOL
        .iter()
        .find(|(tool, _)| *tool == tool_name)
    else {
        return false;
    };
    let mut changed = false;
    walk(args, paths, "", &mut changed);
    changed
}

/// Walk `value`, decoding any string sitting at one of `paths`.
///
/// `path` is the dotted route taken to reach `value` from the argument root.
/// Array indices are not segments, so one spec covers every element. A subtree
/// no spec reaches is pruned rather than descended, which is both the cheap
/// path and the reason a carried payload needs no exclusion list: nothing
/// inside `new_value` or `env` is at a listed path, so the walk never enters
/// them.
fn walk(value: &mut serde_json::Value, paths: &[&str], path: &str, changed: &mut bool) {
    match value {
        serde_json::Value::String(s) => {
            if paths.contains(&path) {
                if let Some(decoded) = decode_xml_entities(s) {
                    *s = decoded;
                    *changed = true;
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                walk(item, paths, path, changed);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if paths.iter().any(|spec| is_at_or_under(spec, &child)) {
                    walk(v, paths, &child, changed);
                }
            }
        }
        _ => {}
    }
}

/// Whether `spec` is `path` itself or something beneath it, i.e. whether
/// descending into `path` could still reach `spec`. The `.` check is what
/// stops `payload_extra` from matching the prefix of `payload.summary`.
fn is_at_or_under(spec: &str, path: &str) -> bool {
    spec.strip_prefix(path)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// The five XML predefined entities plus the two numeric apostrophe forms the
/// model actually emits. Deliberately NOT the full HTML entity table: a label
/// gains nothing from `&copy;` decoding, and every extra name widens the set of
/// literal strings this can corrupt.
const XML_ENTITIES: &[(&str, char)] = &[
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&apos;", '\''),
    ("&#39;", '\''),
    ("&#x27;", '\''),
];

/// Decode the entities of [`XML_ENTITIES`] in ONE left-to-right pass, returning
/// `None` when there was nothing to decode (the overwhelmingly common case, so
/// it costs one scan and no allocation).
///
/// A single pass is the correct decode: `&amp;lt;` yields `&lt;`, not `<`, and
/// `&amp;amp;` yields `&amp;`. Re-running the decoder over its own output would
/// peel a second level and destroy text that was legitimately double-escaped.
fn decode_xml_entities(s: &str) -> Option<String> {
    if !s.contains('&') {
        return None;
    }
    let mut out: Option<String> = None;
    // Byte index of the first byte of `s` not yet copied into `out`. Kept
    // separate from `cursor` so a `&` we decline to decode stays in the
    // uncopied run rather than being dropped: scanning past it must not also
    // consume it.
    let mut copied_to = 0usize;
    let mut cursor = 0usize;
    while let Some(rel) = s[cursor..].find('&') {
        let amp = cursor + rel;
        match XML_ENTITIES
            .iter()
            .find(|(entity, _)| s[amp..].starts_with(entity))
        {
            Some((entity, ch)) => {
                let buf = out.get_or_insert_with(|| String::with_capacity(s.len()));
                buf.push_str(&s[copied_to..amp]);
                buf.push(*ch);
                copied_to = amp + entity.len();
                cursor = copied_to;
            }
            // Not one of ours. Step past the `&` and keep scanning, so
            // `&nbsp;` and a bare `&` both survive untouched.
            None => cursor = amp + '&'.len_utf8(),
        }
    }
    if let Some(buf) = out.as_mut() {
        buf.push_str(&s[copied_to..]);
    }
    out
}

#[cfg(test)]
#[path = "tool_arg_entity_repair_tests.rs"]
mod tests;
