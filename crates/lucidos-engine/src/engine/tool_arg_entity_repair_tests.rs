//! Tests for the tool-argument entity repair. Lifted into a sibling file
//! because they dominate the module (`.claude/rules/rust.md` § Tests).
//!
//! The invariants these pin are named in
//! `docs/plans/2026-08-09-tool-arg-html-entity-repair.md`.

use super::*;
use serde_json::json;

/// The reported bug, end to end through the repair: the seven trigger groups
/// created in one turn on 2026-08-09 landed as `Machine &amp; Tooling Health`.
#[test]
fn the_reported_trigger_group_name_comes_back_literal() {
    let mut args = json!({
        "action": "create",
        "name": "Machine &amp; Tooling Health",
        "order": 6,
    });
    assert!(repair_tool_arg_entities("trigger_groups", &mut args));
    assert_eq!(args["name"], "Machine & Tooling Health");
    // Untouched siblings stay untouched, including the non-string ones.
    assert_eq!(args["action"], "create");
    assert_eq!(args["order"], 6);
}

/// The brief's explicit ask: `& < > " '` all survive, not just `&`. A name
/// carrying any of them would otherwise be corrupted silently.
#[test]
fn every_affected_character_round_trips() {
    let mut args = json!({
        "title": "a &amp; b &lt; c &gt; d &quot;e&quot; f &#39;g&#39; h &apos;i&apos; j &#x27;k&#x27;",
    });
    assert!(repair_tool_arg_entities("send_notification", &mut args));
    assert_eq!(
        args["title"], "a & b < c > d \"e\" f 'g' h 'i' j 'k'",
        "all five XML predefined entities plus both numeric apostrophes decode"
    );
}

/// The load-bearing safety property. These arguments carry HTML the model
/// wrote on purpose, and `&amp;` inside them is correct. Rewriting one would
/// corrupt a file in the user's workspace, which is strictly worse than the bug
/// this module fixes.
#[test]
fn content_bearing_arguments_are_never_rewritten() {
    let html = "<p>Tools &amp; Toys</p>";
    let mut args = json!({
        "path": "apps/logo-studio/index.html",
        "content": html,
        "html_content": html,
        "new_string": "shape &amp; safe-area",
        "old_string": "shape &amp; area",
        "code": "x = a &amp; b",
        "command": "grep 'a &amp; b' f.txt",
        "pattern": "impl.*From&lt;&amp;str&gt;",
        "prompt": "Write a page containing &amp; entities.",
        "text": "&amp;",
        "message": "Restyle the header &amp; footer",
    });
    let before = args.clone();
    assert!(
        repair_tool_arg_entities("write_file", &mut args),
        "the commit message is prose and does repair, so this is not a trivial decline"
    );
    assert_eq!(args["message"], "Restyle the header & footer");
    for key in [
        "path",
        "content",
        "html_content",
        "new_string",
        "old_string",
        "code",
        "command",
        "pattern",
        "prompt",
        "text",
    ] {
        assert_eq!(args[key], before[key], "{key} must be byte-identical");
    }
}

/// One tool call can hold both kinds at once: `create_app` names an app in
/// plain text and ships its markup in the same object. The repair has to split
/// them per key, not per call.
#[test]
fn a_single_call_splits_plain_text_from_markup() {
    let mut args = json!({
        "id": "release-cockpit",
        "name": "Release &amp; Notarization",
        "description": "Walks the release &amp; notarization checklist.",
        "html_content": "<h1>Release &amp; Notarization</h1>",
    });
    assert!(repair_tool_arg_entities("create_app", &mut args));
    assert_eq!(args["name"], "Release & Notarization");
    assert_eq!(
        args["description"],
        "Walks the release & notarization checklist."
    );
    assert_eq!(
        args["html_content"], "<h1>Release &amp; Notarization</h1>",
        "the markup argument keeps its escaping"
    );
}

/// A trigger's intent lives inside `run`, so the walk has to descend rather
/// than only reading the top level. (The `emit_event` payload's `summary`, the
/// other nested case in the brief's sweep list, has its own test below.)
#[test]
fn nested_objects_are_reached() {
    let mut trigger = json!({
        "action": "create",
        "name": "Release &amp; notarize",
        "run": {"type": "intent", "intent": "Watch the notary queue &amp; tell me the verdict"},
    });
    assert!(repair_tool_arg_entities("triggers", &mut trigger));
    assert_eq!(trigger["name"], "Release & notarize");
    assert_eq!(
        trigger["run"]["intent"],
        "Watch the notary queue & tell me the verdict"
    );
    assert_eq!(trigger["run"]["type"], "intent");
}

/// `ask_user_question` nests its plain text two array levels deep, which is the
/// deepest allow-listed shape in the tool surface.
#[test]
fn arrays_of_objects_are_reached() {
    let mut args = json!({
        "questions": [{
            "question": "Ship it &amp; notify?",
            "header": "Ship &amp; tell",
            "options": [
                {"label": "Ship &amp; notify", "description": "Publish &amp; send a push."},
                {"label": "Hold", "description": "Do nothing."},
            ],
        }],
    });
    assert!(repair_tool_arg_entities("ask_user_question", &mut args));
    let q = &args["questions"][0];
    assert_eq!(q["question"], "Ship it & notify?");
    assert_eq!(q["header"], "Ship & tell");
    assert_eq!(q["options"][0]["label"], "Ship & notify");
    assert_eq!(q["options"][0]["description"], "Publish & send a push.");
    assert_eq!(q["options"][1]["label"], "Hold");
}

/// The reason the allow-list is scoped per tool and not per key: `name` is a
/// label in `trigger_groups` and an exact lookup key into
/// `data/config/apis.json` in `proxy_request`. A key-only list read the second
/// as the first, and would leave a proxy entry unresolvable. Both outbound
/// tools are absent from the table, so their whole call, including the
/// third-party body they compose, is left alone.
#[test]
fn an_outbound_tools_lookup_key_is_not_read_as_a_label() {
    let mut proxy = json!({
        "name": "vendor-&amp;-co",
        "path": "/items",
        "body": {"title": "Tools &amp; Toys"},
    });
    let before = proxy.clone();
    assert!(!repair_tool_arg_entities("proxy_request", &mut proxy));
    assert_eq!(
        proxy, before,
        "`name` here resolves an apis.json entry, so rewriting it breaks the lookup"
    );

    let mut http = json!({
        "url": "https://api.example.com/v1/items",
        "method": "POST",
        "body": {"name": "Tools &amp; Toys", "nested": {"description": "A &amp; B"}},
        "headers": {"name": "X-Thing &amp; Co"},
    });
    let before = http.clone();
    assert!(!repair_tool_arg_entities("http_request", &mut http));
    assert_eq!(http, before);

    // Same word, same shape, opposite verdict, because the tool differs.
    let mut group = json!({"action": "create", "name": "vendor-&amp;-co"});
    assert!(repair_tool_arg_entities("trigger_groups", &mut group));
    assert_eq!(group["name"], "vendor-&-co");
}

/// An `emit_event` payload is our own domain event, but only its required
/// `summary` is a documented line of prose. Everything else in it is arbitrary
/// application data that a trigger `condition` later matches against, so the
/// row for `events` names `summary` alone.
#[test]
fn only_the_documented_summary_of_an_event_payload_is_decoded() {
    let mut args = json!({
        "action": "emit",
        "event_type": "ReleaseCompleted",
        "payload": {
            "summary": "Signed &amp; notarized the DMG",
            "name": "AT&amp;T",
            "nested": {"title": "X &amp; Y"},
        },
    });
    assert!(repair_tool_arg_entities("events", &mut args));
    assert_eq!(args["payload"]["summary"], "Signed & notarized the DMG");
    assert_eq!(
        args["payload"]["name"], "AT&amp;T",
        "an arbitrary payload field is stored as the model composed it"
    );
    assert_eq!(args["payload"]["nested"]["title"], "X &amp; Y");
}

/// The specs are full paths, not leaf keys, so a DEEPER field that happens to
/// share a listed name is still arbitrary application data. Only
/// `payload.summary` is the documented one-liner; `payload.details.summary`
/// belongs to whoever designed that event and may be matched by a trigger
/// condition later.
#[test]
fn a_deeper_field_sharing_a_listed_name_is_not_decoded() {
    let mut args = json!({
        "action": "emit",
        "event_type": "ReleaseCompleted",
        "payload": {
            "summary": "Signed &amp; notarized",
            "details": {"summary": "step 3 &amp; 4", "message": "a &amp; b"},
        },
    });
    assert!(repair_tool_arg_entities("events", &mut args));
    assert_eq!(args["payload"]["summary"], "Signed & notarized");
    assert_eq!(
        args["payload"]["details"]["summary"], "step 3 &amp; 4",
        "a nested summary is somebody else's field, not the documented one"
    );
    assert_eq!(args["payload"]["details"]["message"], "a &amp; b");
}

/// The prune step matches on path SEGMENTS. A sibling key that merely starts
/// with a listed path's text must not be mistaken for it, or the walk would
/// wander into an unrelated subtree.
#[test]
fn a_sibling_sharing_a_path_prefix_is_not_mistaken_for_it() {
    assert!(is_at_or_under("payload.summary", "payload"));
    assert!(is_at_or_under("payload.summary", "payload.summary"));
    assert!(!is_at_or_under("payload.summary", "payload_extra"));
    assert!(!is_at_or_under("payload.summary", "payloads"));
    assert!(!is_at_or_under("name", "namespace"));

    let mut args = json!({
        "action": "emit",
        "event_type": "X",
        "payload_extra": {"summary": "a &amp; b"},
        "payload": {"summary": "c &amp; d"},
    });
    assert!(repair_tool_arg_entities("events", &mut args));
    assert_eq!(args["payload"]["summary"], "c & d");
    assert_eq!(args["payload_extra"]["summary"], "a &amp; b");
}

/// `edit_file`'s JSON mode writes `new_value` verbatim into a `.json` or
/// `.slides` file in the user's workspace, so a nested `name` inside it is the
/// user's stored data, not our label. Decoding it would silently rewrite the
/// file: the same corruption class as touching `content`, reached through an
/// object instead of a string.
#[test]
fn the_edit_file_json_replacement_value_is_stored_data_not_a_label() {
    let mut args = json!({
        "path": "artifacts/work-tracker/data.json",
        "json_path": "vendors[0]",
        "new_value": {"name": "AT&amp;T", "tags": ["A &amp; B"], "sub": {"title": "X &amp; Y"}},
        "message": "Add the vendor",
    });
    let before = args["new_value"].clone();
    assert!(
        !repair_tool_arg_entities("edit_file", &mut args),
        "nothing outside new_value carries an entity here"
    );
    assert_eq!(
        args["new_value"], before,
        "the replacement value must reach the file byte-identical"
    );
}

/// Two payloads the model composes for somewhere else: an MCP server's
/// environment map, and a trigger's event-payload matcher. Neither needs an
/// exclusion list, because `env.name` and `on.condition.name` are simply not
/// the listed paths (`name`, `run.intent`).
#[test]
fn carried_containers_are_not_descended_into() {
    let mut mcp = json!({
        "action": "setup",
        "id": "blender-mcp",
        "name": "Blender &amp; Friends",
        "env": {"name": "a &amp; b", "TOKEN": "x &amp; y"},
    });
    assert!(
        repair_tool_arg_entities("mcp", &mut mcp),
        "the built-in `mcp` management tool is ours: the mcp__ gate must not swallow it"
    );
    assert_eq!(mcp["name"], "Blender & Friends", "the label still repairs");
    assert_eq!(mcp["env"]["name"], "a &amp; b");
    assert_eq!(mcp["env"]["TOKEN"], "x &amp; y");

    // Through `triggers`, which DOES allow-list `name`, so the condition is
    // skipped by its own key rather than by the tool being absent.
    let mut trigger = json!({
        "action": "create",
        "name": "Watch groups &amp; report",
        "on": [{"event_type": "TriggerGroupCreated", "condition": {"name": "A &amp; B"}}],
    });
    assert!(repair_tool_arg_entities("triggers", &mut trigger));
    assert_eq!(trigger["name"], "Watch groups & report");
    assert_eq!(
        trigger["on"][0]["condition"]["name"], "A &amp; B",
        "a matcher is compared against stored rows, so it stays as written"
    );
}

/// The same word at two depths in one call: `edit_file`'s own commit message
/// repairs, and the `message` inside the JSON value it writes to disk does
/// not, because that one is at `new_value.message`.
#[test]
fn a_carried_container_does_not_shield_its_siblings() {
    let mut args = json!({
        "path": "artifacts/work-tracker/data.json",
        "json_path": "vendors[0]",
        "new_value": {"message": "left &amp; alone"},
        "message": "Add Docker Hub &amp; GitHub",
    });
    assert!(repair_tool_arg_entities("edit_file", &mut args));
    assert_eq!(args["message"], "Add Docker Hub & GitHub");
    assert_eq!(args["new_value"]["message"], "left &amp; alone");
}

/// An `mcp__<server>__<tool>` call runs against a schema somebody else wrote,
/// so none of the per-argument reasoning behind the allow-list applies to it:
/// a third-party `name` may be an identifier, a template, or a value the
/// server needs escaped. Declining leaves the model's entities in place, which
/// is the lesser harm.
#[test]
fn a_third_party_mcp_tool_is_declined_whole() {
    let mut args = json!({
        "name": "Tools &amp; Toys",
        "title": "A &amp; B",
        "payload": {"summary": "x &amp; y"},
    });
    let before = args.clone();
    assert!(!repair_tool_arg_entities(
        "mcp__example_server__create_issue",
        &mut args
    ));
    assert_eq!(args, before);
}

/// One pass, not a loop to a fixed point. A doubly-escaped value peels exactly
/// one level, which is what a decoder owes: peeling twice would destroy text
/// that was legitimately escaped once.
#[test]
fn decoding_peels_exactly_one_level() {
    assert_eq!(decode_xml_entities("&amp;lt;").unwrap(), "&lt;");
    assert_eq!(decode_xml_entities("&amp;amp;").unwrap(), "&amp;");
    assert_eq!(
        decode_xml_entities("&amp;quot;x&amp;quot;").unwrap(),
        "&quot;x&quot;"
    );
}

/// Entities we do not own, and a bare `&`, pass through byte-identical. The
/// scanner steps over a declined `&` without consuming it, so the text on
/// either side of one survives.
#[test]
fn unknown_entities_and_bare_ampersands_survive() {
    assert_eq!(decode_xml_entities("Tools & Toys"), None);
    assert_eq!(decode_xml_entities("a &nbsp; b"), None);
    assert_eq!(decode_xml_entities("&copy; 2026"), None);
    assert_eq!(
        decode_xml_entities("a &nbsp; b &amp; c").unwrap(),
        "a &nbsp; b & c",
        "text before a declined entity must not be dropped"
    );
    assert_eq!(
        decode_xml_entities("&amp; a &nbsp; b").unwrap(),
        "& a &nbsp; b",
        "text after a declined entity must not be dropped"
    );
}

/// A string with no `&` at all is the overwhelmingly common case and returns
/// `None` without allocating, so the repair costs one scan per allow-listed
/// argument.
#[test]
fn a_clean_string_allocates_nothing() {
    assert_eq!(decode_xml_entities("Demo Production"), None);
    let mut args = json!({"name": "Demo Production", "order": 7});
    assert!(
        !repair_tool_arg_entities("trigger_groups", &mut args),
        "a clean call reports no change, so the caller logs nothing"
    );
}

/// Multi-byte text either side of an entity: the scanner indexes by byte and
/// must not slice off a char boundary.
#[test]
fn multibyte_text_around_an_entity_is_safe() {
    assert_eq!(
        decode_xml_entities("Ærlig &amp; grei, på norsk").unwrap(),
        "Ærlig & grei, på norsk"
    );
    assert_eq!(
        decode_xml_entities("emoji 🎉 &lt;tag&gt;").unwrap(),
        "emoji 🎉 <tag>"
    );
}

/// Anything that is not an object or an array is returned untouched, including
/// a bare string at the root: an argument value only earns decoding by being
/// reached through an allow-listed key.
#[test]
fn a_non_object_root_is_left_alone() {
    for mut v in [
        json!("Tools &amp; Toys"),
        json!(42),
        json!(true),
        json!(null),
        json!(["Tools &amp; Toys"]),
    ] {
        let before = v.clone();
        assert!(!repair_tool_arg_entities("trigger_groups", &mut v));
        assert_eq!(v, before);
    }
}
