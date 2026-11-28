use super::*;
use serde_json::json;

#[test]
fn the_reported_trigger_group_name_comes_back_literal() {
    let mut args = json!({
        "action": "create",
        "name": "Machine &amp; Tooling Health",
        "order": 6,
    });
    assert!(repair_tool_arg_entities("trigger_groups", &mut args));
    assert_eq!(args["name"], "Machine & Tooling Health");
    assert_eq!(args["action"], "create");
    assert_eq!(args["order"], 6);
}

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

    let mut group = json!({"action": "create", "name": "vendor-&amp;-co"});
    assert!(repair_tool_arg_entities("trigger_groups", &mut group));
    assert_eq!(group["name"], "vendor-&-co");
}

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

#[test]
fn decoding_peels_exactly_one_level() {
    assert_eq!(decode_xml_entities("&amp;lt;").unwrap(), "&lt;");
    assert_eq!(decode_xml_entities("&amp;amp;").unwrap(), "&amp;");
    assert_eq!(
        decode_xml_entities("&amp;quot;x&amp;quot;").unwrap(),
        "&quot;x&quot;"
    );
}

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

#[test]
fn a_clean_string_allocates_nothing() {
    assert_eq!(decode_xml_entities("Demo Production"), None);
    let mut args = json!({"name": "Demo Production", "order": 7});
    assert!(
        !repair_tool_arg_entities("trigger_groups", &mut args),
        "a clean call reports no change, so the caller logs nothing"
    );
}

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
