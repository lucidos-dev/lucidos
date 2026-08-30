use super::*;
use serde_json::json;

#[test]
fn claude_code_idled_has_changes_serialization() {
    // With has_changes=true → field included
    let event = ThreadEvent::CodingAgentIdled {
        has_changes: true,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["type"], "CodingAgentIdled");
    assert_eq!(serialized["has_changes"], true);

    // With has_changes=false → field skipped (skip_serializing_if = "is_false")
    let event2 = ThreadEvent::CodingAgentIdled {
        has_changes: false,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    };
    let serialized2 = serde_json::to_value(&event2).unwrap();
    assert_eq!(serialized2["type"], "CodingAgentIdled");
    assert!(
        serialized2.get("has_changes").is_none(),
        "false has_changes should be skipped"
    );

    // Old DB format without has_changes deserializes with default=false
    let old_format: ThreadEvent = serde_json::from_str(r#"{"type":"CodingAgentIdled"}"#).unwrap();
    match old_format {
        ThreadEvent::CodingAgentIdled { has_changes, .. } => assert!(!has_changes),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn tool_called_description_serialization() {
    // With description → included in JSON
    let event = ThreadEvent::ToolCalled {
        name: "read_file".into(),
        args: json!({"path": "test.txt"}),
        description: "Reading test.txt...".into(),
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["description"], "Reading test.txt...");

    // Empty description → skipped (skip_serializing_if = "is_empty_str")
    let event2 = ThreadEvent::ToolCalled {
        name: "read_file".into(),
        args: json!({"path": "test.txt"}),
        description: String::new(),
    };
    let serialized2 = serde_json::to_value(&event2).unwrap();
    assert!(
        serialized2.get("description").is_none(),
        "empty description should be skipped"
    );
}

#[test]
fn tool_called_backward_compat_no_description() {
    // Old DB rows without description field must still deserialize
    let old_format: ThreadEvent = serde_json::from_str(
        r#"{"type":"ToolCalled","name":"read_file","args":{"path":"test.txt"}}"#,
    )
    .unwrap();
    match old_format {
        ThreadEvent::ToolCalled {
            name, description, ..
        } => {
            assert_eq!(name, "read_file");
            assert!(
                description.is_empty(),
                "missing description should default to empty string"
            );
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn cc_tool_called_description_serialization() {
    let event = ThreadEvent::CodingAgentToolCalled {
        name: "Read".into(),
        args: json!({"file_path": "/src/main.rs"}),
        description: "Read main.rs".into(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        tool_use_id: String::new(),
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["description"], "Read main.rs");

    // Empty → skipped
    let event2 = ThreadEvent::CodingAgentToolCalled {
        name: "Read".into(),
        args: json!({}),
        description: String::new(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        tool_use_id: String::new(),
    };
    let serialized2 = serde_json::to_value(&event2).unwrap();
    assert!(serialized2.get("description").is_none());
}

#[test]
fn cc_tool_called_result_tool_use_id_round_trip() {
    let call = ThreadEvent::CodingAgentToolCalled {
        name: "Bash".into(),
        args: json!({"command": "ls"}),
        description: "ls".into(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        tool_use_id: "toolu_42".into(),
    };
    let serialized = serde_json::to_value(&call).unwrap();
    assert_eq!(serialized["tool_use_id"], "toolu_42");

    // Empty id → skipped from the wire
    let call_no_id = ThreadEvent::CodingAgentToolCalled {
        name: "Bash".into(),
        args: json!({}),
        description: String::new(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        tool_use_id: String::new(),
    };
    assert!(serde_json::to_value(&call_no_id)
        .unwrap()
        .get("tool_use_id")
        .is_none());

    // Legacy DB row without tool_use_id deserializes cleanly
    let legacy: ThreadEvent =
        serde_json::from_str(r#"{"type":"CodingAgentToolResult","name":"","result":"ok"}"#)
            .unwrap();
    match legacy {
        ThreadEvent::CodingAgentToolResult { tool_use_id, .. } => {
            assert!(tool_use_id.is_empty());
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn cc_tool_called_backward_compat_no_description() {
    let old_format: ThreadEvent =
        serde_json::from_str(r#"{"type":"CodingAgentToolCalled","name":"Read","args":{}}"#)
            .unwrap();
    match old_format {
        ThreadEvent::CodingAgentToolCalled {
            name, description, ..
        } => {
            assert_eq!(name, "Read");
            assert!(description.is_empty());
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn message_received_with_image_hashes() {
    let event = ThreadEvent::MessageReceived {
        voice_session_id: None,
        text: "look at this".into(),
        user_image_hashes: vec!["abcd1234".into(), "ef567890".into()],
        device_id: Some("phone-1".into()),
        device: Some("Test iPhone".into()),
        image_description: Some("a cat".into()),
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    };
    let payload = event.to_payload(&EventMeta::NONE);
    assert_eq!(payload["text"], "look at this");
    // Hashes are stored as a flat string array — no inline base64 anywhere.
    assert_eq!(payload["user_image_hashes"][0], "abcd1234");
    assert_eq!(payload["user_image_hashes"][1], "ef567890");
    assert!(
        payload.get("images").is_none(),
        "legacy `images` field must not appear in the new shape"
    );
    assert_eq!(payload["device_id"], "phone-1");
    assert_eq!(payload["image_description"], "a cat");
}

#[test]
fn message_received_without_optional_fields() {
    let event = ThreadEvent::MessageReceived {
        voice_session_id: None,
        text: "hello".into(),
        user_image_hashes: vec![],
        device_id: None,
        device: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    };
    let payload = event.to_payload(&EventMeta::NONE);
    assert_eq!(payload["text"], "hello");
    // Optional fields should be absent
    assert!(
        payload.get("user_image_hashes").is_none(),
        "empty user_image_hashes should be skipped"
    );
    assert!(
        payload.get("device_id").is_none(),
        "None device_id should be skipped"
    );
}

#[test]
fn trigger_started_with_details() {
    let event = ThreadEvent::TriggerStarted {
        trigger_id: "t-1".into(),
        trigger_name: Some("daily-report".into()),
        prompt: Some("Run the daily report".into()),
        invocation: Some(TriggerInvocation::Schedule),
        origin: None,
        go_to_review: false,
        model: None,
        reasoning_effort: None,
    };
    let payload = event.to_payload(&EventMeta::NONE);
    assert_eq!(payload["trigger_id"], "t-1");
    assert_eq!(payload["trigger_name"], "daily-report");
    assert_eq!(payload["prompt"], "Run the daily report");
    assert_eq!(payload["invocation"]["kind"], "Schedule");
}

#[test]
fn trigger_started_event_invocation_serializes_event_type_and_id() {
    let event_id = uuid::Uuid::new_v4();
    let event = ThreadEvent::TriggerStarted {
        trigger_id: "t-2".into(),
        trigger_name: Some("sleep-import".into()),
        prompt: Some("Import overnight sleep data".into()),
        invocation: Some(TriggerInvocation::Event {
            event_type: "DataImported".into(),
            event_id: Some(event_id),
            thread_id: None,
        }),
        origin: None,
        go_to_review: false,
        model: None,
        reasoning_effort: None,
    };
    let payload = event.to_payload(&EventMeta::NONE);
    assert_eq!(payload["invocation"]["kind"], "Event");
    assert_eq!(payload["invocation"]["event_type"], "DataImported");
    assert_eq!(payload["invocation"]["event_id"], event_id.to_string());
}

#[test]
fn trigger_started_legacy_task_id_alias_deserializes() {
    // Old DB rows persisted before the rename used `task_id`/`task_name`.
    // The migration renames event_type values, but field names live in the
    // jsonb payload and must continue to deserialize via serde aliases so
    // historical rows replay cleanly.
    let json = r#"{"type":"TriggerStarted","task_id":"old","task_name":"legacy"}"#;
    let event: ThreadEvent = serde_json::from_str(json).unwrap();
    match event {
        ThreadEvent::TriggerStarted {
            trigger_id,
            trigger_name,
            ..
        } => {
            assert_eq!(trigger_id, "old");
            assert_eq!(trigger_name.as_deref(), Some("legacy"));
        }
        _ => panic!("expected TriggerStarted"),
    }
}

#[test]
fn change_proposed_new_format() {
    let event = ThreadEvent::ChangeProposed {
        change_id: "c-1".into(),
        description: Some("Fix the bug".into()),
        files: vec!["src/main.rs".into()],
        requires_restart: true,
        origin: None,
        commit_sha: None,
        branch_name: String::new(),
        repo_root: String::new(),
        hardened: false,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    };
    let payload = event.to_payload(&EventMeta::NONE);
    assert_eq!(payload["change_id"], "c-1");
    assert_eq!(payload["description"], "Fix the bug");
    assert_eq!(payload["requires_restart"], true);
    // Legacy fields should be absent (empty → skipped); `incomplete: false`
    // is the default and must also be skipped so legacy DB rows decode
    // without a wire-shape diff.
    assert!(payload.get("path").is_none());
    assert!(payload.get("diff").is_none());
    assert!(
        payload.get("incomplete").is_none(),
        "incomplete=false (the common case) must skip serialization to keep \
             new event payloads byte-compatible with pre-field DB rows"
    );
}
