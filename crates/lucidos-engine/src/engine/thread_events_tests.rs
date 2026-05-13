use super::*;
use serde_json::json;

#[test]
fn message_origin_engine_serializes_with_kind_engine() {
    let origin = MessageOrigin::Engine {
        reason: EngineReason::ContinuationStarted,
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json["kind"], "engine");
    assert_eq!(json["reason"]["kind"], "continuation_started");
}

/// `EngineReason` legacy serde alias: old DB rows persisted with
/// `{"kind":"session_recovered"}` (and the even older
/// `{"kind":"session_resumed"}` if any survive) must still deserialize as the
/// renamed `ContinuationStarted` variant. Without the alias the projection
/// crashes on any historical row using the old name.
#[test]
fn engine_reason_continuation_started_accepts_legacy_session_recovered_alias() {
    let v: EngineReason = serde_json::from_str(r#"{"kind":"session_recovered"}"#).unwrap();
    assert_eq!(v, EngineReason::ContinuationStarted);
}

/// `MessageOrigin::System` serializes as `{"kind":"system"}` with NO
/// other fields. The frontend's MessageOrigin union has `{ kind: 'system' }`
/// (no reason / no metadata) — adding fields here would break that contract.
/// Distinct from Engine: System means the host killed the process; Engine
/// means the engine deliberately took an action.
#[test]
fn message_origin_system_serializes_with_kind_system_no_other_fields() {
    let origin = MessageOrigin::System;
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json, serde_json::json!({"kind": "system"}));
}

/// System is intrinsically engine-mode (deterministic, non-human, non-agent).
/// Mirrors `MessageOrigin::Engine`'s mode — the chip differentiates via
/// label override (System vs Lucidos Engine), not via mode.
#[test]
fn message_origin_system_mode_is_engine() {
    assert_eq!(MessageOrigin::System.mode(), ActorMode::Engine);
}

/// `MessageOrigin::system()` is the canonical constructor — emit sites use
/// it for the "host killed the process" attribution (orphan recovery,
/// shutdown, safety net, post-restart abort marker).
#[test]
fn message_origin_system_constructor() {
    assert!(matches!(MessageOrigin::system(), MessageOrigin::System));
}

#[test]
fn message_origin_engine_scheduler_carries_trigger_metadata() {
    let trigger_id = uuid::Uuid::new_v4().to_string();
    let origin = MessageOrigin::Engine {
        reason: EngineReason::Scheduler {
            trigger_id: trigger_id.clone(),
            trigger_name: Some("nightly-backup".to_string()),
        },
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json["reason"]["kind"], "scheduler");
    assert_eq!(json["reason"]["trigger_id"], trigger_id);
    assert_eq!(json["reason"]["trigger_name"], "nightly-backup");
}

#[test]
fn message_origin_engine_round_trips_through_serde() {
    let original = MessageOrigin::Engine {
        reason: EngineReason::HardenRetrigger,
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn actor_mode_serializes_lowercase_strings() {
    assert_eq!(
        serde_json::to_string(&ActorMode::Human).unwrap(),
        "\"human\""
    );
    assert_eq!(
        serde_json::to_string(&ActorMode::Agent).unwrap(),
        "\"agent\""
    );
    assert_eq!(
        serde_json::to_string(&ActorMode::Engine).unwrap(),
        "\"engine\""
    );
}

#[test]
fn actor_mode_deserializes_lowercase_strings() {
    assert_eq!(
        serde_json::from_str::<ActorMode>("\"human\"").unwrap(),
        ActorMode::Human
    );
    assert_eq!(
        serde_json::from_str::<ActorMode>("\"agent\"").unwrap(),
        ActorMode::Agent
    );
    assert_eq!(
        serde_json::from_str::<ActorMode>("\"engine\"").unwrap(),
        ActorMode::Engine
    );
}

#[test]
fn message_origin_thread_link_defaults_mode_to_agent_when_missing() {
    let json = r#"{
            "kind": "thread_link",
            "thread_id": "00000000-0000-0000-0000-000000000001"
        }"#;
    let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
    match parsed {
        MessageOrigin::ThreadLink {
            mode, direction, ..
        } => {
            assert_eq!(mode, ActorMode::Agent);
            assert_eq!(direction, ThreadDirection::Parent);
        }
        other => panic!("expected ThreadLink, got {:?}", other),
    }
}

/// Historical DB rows persisted under the old variant name. The
/// `serde(alias = "parent_thread")` + default `direction` keep them
/// readable as `ThreadLink { direction: Parent }`.
#[test]
fn message_origin_legacy_parent_thread_kind_deserializes_as_thread_link() {
    let json = r#"{
            "kind": "parent_thread",
            "thread_id": "00000000-0000-0000-0000-000000000001",
            "mode": "engine"
        }"#;
    let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
    match parsed {
        MessageOrigin::ThreadLink {
            mode, direction, ..
        } => {
            assert_eq!(mode, ActorMode::Engine);
            assert_eq!(direction, ThreadDirection::Parent);
        }
        other => panic!(
            "expected ThreadLink (from parent_thread alias), got {:?}",
            other
        ),
    }
}

#[test]
fn message_origin_workspace_defaults_mode_to_human_when_missing() {
    let json = r#"{ "kind": "workspace", "workspace": "personal" }"#;
    let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
    match parsed {
        MessageOrigin::Workspace { mode, .. } => assert_eq!(mode, ActorMode::Human),
        other => panic!("expected Workspace, got {:?}", other),
    }
}

#[test]
fn message_origin_thread_link_round_trips_with_explicit_engine_mode() {
    let original = MessageOrigin::ThreadLink {
        thread_id: uuid::Uuid::new_v4(),
        title: Some("recovered".into()),
        spawning_event_id: None,
        mode: ActorMode::Engine,
        direction: ThreadDirection::Parent,
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn message_origin_thread_link_child_round_trips() {
    let original = MessageOrigin::ThreadLink {
        thread_id: uuid::Uuid::new_v4(),
        title: Some("child task".into()),
        spawning_event_id: Some(uuid::Uuid::new_v4()),
        mode: ActorMode::Agent,
        direction: ThreadDirection::Child,
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn message_origin_mode_derives_human_for_device_and_api() {
    let device = MessageOrigin::Device {
        device_id: "d".into(),
        label: "l".into(),
    };
    let api = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Human,
    };
    assert_eq!(device.mode(), ActorMode::Human);
    assert_eq!(api.mode(), ActorMode::Human);
}

#[test]
fn message_origin_mode_derives_engine_for_engine_variant() {
    let origin = MessageOrigin::Engine {
        reason: EngineReason::ContinuationStarted,
    };
    assert_eq!(origin.mode(), ActorMode::Engine);
}

#[test]
fn message_origin_mode_reads_field_for_workspace_and_thread_link() {
    let ws = MessageOrigin::Workspace {
        workspace: "x".into(),
        thread_id: None,
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
    };
    let tl = MessageOrigin::ThreadLink {
        thread_id: uuid::Uuid::new_v4(),
        title: None,
        spawning_event_id: None,
        mode: ActorMode::Engine,
        direction: ThreadDirection::Parent,
    };
    assert_eq!(ws.mode(), ActorMode::Agent);
    assert_eq!(tl.mode(), ActorMode::Engine);
}

#[test]
fn thread_event_serializes_with_type_tag() {
    let event = ThreadEvent::ToolCalled {
        name: "read_file".to_string(),
        args: json!({"path": "test.txt"}),
        description: "Reading test.txt...".to_string(),
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["type"], "ToolCalled");
    assert_eq!(serialized["name"], "read_file");
    assert_eq!(serialized["args"]["path"], "test.txt");
    assert_eq!(serialized["description"], "Reading test.txt...");
}

#[test]
fn thread_event_type_name_extraction() {
    let cases: Vec<(ThreadEvent, &str)> = vec![
        (
            ThreadEvent::MessageReceived {
                text: "hi".into(),
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
            },
            "MessageReceived",
        ),
        (
            ThreadEvent::TextStreamed { text: "t".into() },
            "TextStreamed",
        ),
        (ThreadEvent::Thinking { text: "hmm".into() }, "Thinking"),
        (
            ThreadEvent::MemorySearched {
                results: 5,
                queries: vec!["birthday".into()],
            },
            "MemorySearched",
        ),
        (
            ThreadEvent::ToolCalled {
                name: "x".into(),
                args: json!({}),
                description: String::new(),
            },
            "ToolCalled",
        ),
        (
            ThreadEvent::ToolResult {
                name: "x".into(),
                result: "ok".into(),
                images: vec![],
                success: true,
            },
            "ToolResult",
        ),
        (
            ThreadEvent::ResponseGenerated {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            "ResponseGenerated",
        ),
        (
            ThreadEvent::ResponseCanceled {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause: crate::engine::thread_events::CancelCause::UserStop,
            },
            "ResponseCanceled",
        ),
        (
            ThreadEvent::ResponseAborted {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause: crate::engine::thread_events::AbortCause::EngineShutdown,
            },
            "ResponseAborted",
        ),
        (
            ThreadEvent::ResponseFailed { error: "e".into() },
            "ResponseFailed",
        ),
        (
            ThreadEvent::ContinuationStarted {
                branch: String::new(),
                origin: None,
            },
            "ContinuationStarted",
        ),
        (
            ThreadEvent::SessionStarted {
                session_id: "s".into(),
                branch: String::new(),
                repo_id: None,
            },
            "SessionStarted",
        ),
        (
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
            "SessionEnded",
        ),
        (
            ThreadEvent::CodingAgentTextStreamed {
                text: "t".into(),
                agent: crate::runtime::AgentKind::ClaudeCode,
            },
            "CodingAgentTextStreamed",
        ),
        (
            ThreadEvent::CodingAgentToolCalled {
                name: "n".into(),
                args: json!({}),
                description: String::new(),
                agent: crate::runtime::AgentKind::ClaudeCode,
                tool_use_id: String::new(),
            },
            "CodingAgentToolCalled",
        ),
        (
            ThreadEvent::CodingAgentToolResult {
                name: "n".into(),
                result: "r".into(),
                agent: crate::runtime::AgentKind::ClaudeCode,
                tool_use_id: String::new(),
            },
            "CodingAgentToolResult",
        ),
        (
            ThreadEvent::CodingAgentUserMessageSent {
                text: "t".into(),
                agent: crate::runtime::AgentKind::ClaudeCode,
            },
            "CodingAgentUserMessageSent",
        ),
        (
            ThreadEvent::MissingHardeningDetected { origin: None },
            "MissingHardeningDetected",
        ),
        (
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: None,
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
            },
            "CodingAgentIdled",
        ),
        (
            ThreadEvent::ThreadTitleGenerated { title: "t".into() },
            "ThreadTitleGenerated",
        ),
        (
            ThreadEvent::ThreadTitleRenamed {
                title: "new".into(),
            },
            "ThreadTitleRenamed",
        ),
        (ThreadEvent::ThreadSaved, "ThreadSaved"),
        (ThreadEvent::ThreadUnsaved, "ThreadUnsaved"),
        (ThreadEvent::ThreadArchived, "ThreadArchived"),
        (
            ThreadEvent::TriggerStarted {
                trigger_id: "id".into(),
                trigger_name: None,
                prompt: None,
                invocation: None,
                origin: None,
                go_to_review: false,
            },
            "TriggerStarted",
        ),
        (
            ThreadEvent::TriggerCompleted {
                trigger_id: "id".into(),
                trigger_name: None,
                result_summary: None,
            },
            "TriggerCompleted",
        ),
        (
            ThreadEvent::ChangeProposed {
                change_id: "c".into(),
                description: None,
                files: vec![],
                requires_restart: false,
                origin: None,
                commit_sha: None,
                branch_name: String::new(),
                repo_root: String::new(),
                hardened: false,
                incomplete: false,
                path: String::new(),
                diff: String::new(),
            },
            "ChangeProposed",
        ),
        (
            ThreadEvent::ChangeApplied {
                change_id: "c".into(),
                requires_restart: false,
                client_update: false,
                commits: vec![],
                thread_title: None,
                actor: None,
                pre_merge_sha: None,
                post_merge_sha: None,
                path: String::new(),
            },
            "ChangeApplied",
        ),
        (
            ThreadEvent::ChangeDiscarded {
                change_id: "c".into(),
                actor: None,
                path: String::new(),
            },
            "ChangeDiscarded",
        ),
        (
            ThreadEvent::ChangeReverted {
                change_id: "c".into(),
                actor: None,
                path: String::new(),
            },
            "ChangeReverted",
        ),
        (
            ThreadEvent::ChangeApplyFailed {
                change_id: "c".into(),
                error: "conflict".into(),
                actor: None,
            },
            "ChangeApplyFailed",
        ),
        (
            ThreadEvent::MergeConflictDetected {
                change_id: "c".into(),
                files: vec!["file.rs".into()],
                origin: None,
            },
            "MergeConflictDetected",
        ),
        (
            ThreadEvent::MergeResolutionStarted {
                change_id: "c".into(),
                worktree_path: "/tmp/wt".into(),
                temp_branch: "merge-tmp/c".into(),
            },
            "MergeResolutionStarted",
        ),
        (
            ThreadEvent::MergeResolutionCleared {
                change_id: "c".into(),
            },
            "MergeResolutionCleared",
        ),
        (
            ThreadEvent::ChangeHardened {
                change_id: "c".into(),
                actor: None,
            },
            "ChangeHardened",
        ),
        (
            ThreadEvent::CredentialRequested {
                provider: "github".into(),
            },
            "CredentialRequested",
        ),
        (
            ThreadEvent::McpConsentRequested {
                tool: "t".into(),
                args: json!({}),
            },
            "McpConsentRequested",
        ),
    ];
    for (event, expected) in cases {
        assert_eq!(
            event.event_type(),
            expected,
            "event_type() mismatch for {:?}",
            event
        );
    }
}

#[test]
fn transient_event_serializes() {
    let event = ThreadEvent::Retrying {
        reason: "rate limited".to_string(),
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["type"], "Retrying");
    assert_eq!(serialized["reason"], "rate limited");

    let event2 = ThreadEvent::PreambleCompleting;
    let serialized2 = serde_json::to_value(&event2).unwrap();
    assert_eq!(serialized2["type"], "PreambleCompleting");
}

#[test]
fn all_db_event_types_have_variants() {
    // Every known DB event_type string must round-trip through serde deserialization.
    // Old format (unit variants) and new format (struct variants) must both work.
    let known_types = vec![
        // Chat
        r#"{"type":"MessageReceived","text":"hi"}"#,
        r#"{"type":"TextStreamed","text":"t"}"#,
        r#"{"type":"Thinking","text":"hmm"}"#,
        r#"{"type":"Thinking","text":"ctx","context_tokens":1000,"context_messages":5,"trimmed":true}"#,
        r#"{"type":"MemorySearched","results":3}"#,
        r#"{"type":"MemorySearched","results":5,"queries":["birthday","date of birth"]}"#,
        r#"{"type":"ToolCalled","name":"x","args":{}}"#,
        r#"{"type":"ToolResult","name":"x","result":"ok"}"#,
        // Legacy event types (ContextAssembled, ContextTokensMeasured) retired
        // — the test below covers ContextCaptured. Old DB rows replay through
        // the frontend's `synthesizeContextCapture` shim, not via
        // `ThreadEvent` deserialization.
        // Old format (unit) — must still deserialize
        r#"{"type":"ResponseGenerated"}"#,
        r#"{"type":"ResponseCanceled"}"#,
        // New format (struct)
        r#"{"type":"ResponseGenerated","text":"answer","images":["img.png"]}"#,
        r#"{"type":"ResponseCanceled","text":"partial","images":[]}"#,
        // With typed cause (new wire field)
        r#"{"type":"ResponseCanceled","text":"partial","cause":"user_stop"}"#,
        // Legacy CancelCause `stale_settle` (now an AbortCause) — old DB rows
        // must still deserialize via `#[serde(other)] Unknown` fallback.
        r#"{"type":"ResponseCanceled","cause":"stale_settle"}"#,
        r#"{"type":"ResponseAborted"}"#,
        r#"{"type":"ResponseAborted","text":"partial","images":[]}"#,
        r#"{"type":"ResponseAborted","cause":"engine_shutdown"}"#,
        r#"{"type":"ResponseAborted","cause":"safety_net"}"#,
        r#"{"type":"ResponseAborted","cause":"stale_settle"}"#,
        r#"{"type":"ResponseFailed","error":"e"}"#,
        // Resume-after-abort boundary
        r#"{"type":"ContinuationStarted","branch":"claude-code/20260318"}"#,
        r#"{"type":"ContinuationStarted"}"#,
        // Legacy: old events stored as SessionRecovered / SessionResumed must
        // still deserialize via the serde aliases on ContinuationStarted.
        r#"{"type":"SessionRecovered","branch":"claude-code/20260318"}"#,
        r#"{"type":"SessionRecovered"}"#,
        r#"{"type":"SessionResumed","branch":"claude-code/20260318"}"#,
        r#"{"type":"SessionResumed"}"#,
        r#"{"type":"SessionStarted","session_id":"s"}"#,
        r#"{"type":"SessionStarted","session_id":"s","branch":"claude-code/20260318"}"#,
        // New format with repo_id for external repo binding
        r#"{"type":"SessionStarted","session_id":"s","branch":"claude-code/20260318","repo_id":"550e8400-e29b-41d4-a716-446655440000"}"#,
        r#"{"type":"SessionEnded"}"#,
        // New format with reason
        r#"{"type":"SessionEnded","reason":"user_ended"}"#,
        r#"{"type":"SessionEnded","reason":"changes_proposed"}"#,
        r#"{"type":"SessionEnded","reason":"changes_applied"}"#,
        r#"{"type":"SessionEnded","reason":"auto_ended"}"#,
        r#"{"type":"CodingAgentTextStreamed","text":"t"}"#,
        r#"{"type":"CodingAgentToolCalled","name":"n","args":{}}"#,
        r#"{"type":"CodingAgentToolResult","name":"n","result":"r"}"#,
        r#"{"type":"CodingAgentUserMessageSent","text":"t"}"#,
        r#"{"type":"MissingHardeningDetected"}"#,
        r#"{"type":"CodingAgentIdled"}"#,
        // New format with has_changes
        r#"{"type":"CodingAgentIdled","has_changes":true}"#,
        // Thread lifecycle
        r#"{"type":"ThreadTitleGenerated","title":"t"}"#,
        r#"{"type":"ThreadTitleRenamed","title":"new title"}"#,
        r#"{"type":"ThreadSaved"}"#,
        r#"{"type":"ThreadUnsaved"}"#,
        r#"{"type":"ThreadArchived"}"#,
        // EventMeta.actor merged into payload — must round-trip on unit and
        // struct variants alike. Internally-tagged enums tolerate extra
        // fields by default, but make it a regression test so a future
        // `#[serde(deny_unknown_fields)]` flip would fail loudly here.
        r#"{"type":"ThreadSaved","actor":{"kind":"device","device_id":"d","label":"Chrome"}}"#,
        r#"{"type":"ThreadUnsaved","actor":{"kind":"api","user_agent":"curl/8"}}"#,
        r#"{"type":"ThreadArchived","actor":{"kind":"workspace","workspace":"dev"}}"#,
        r#"{"type":"ThreadTitleRenamed","title":"x","actor":{"kind":"device","device_id":"d","label":"l"}}"#,
        // Triggers — minimal + full + legacy task_id alias on the renamed variant
        r#"{"type":"TriggerStarted","trigger_id":"id"}"#,
        r#"{"type":"TriggerStarted","trigger_id":"id","trigger_name":"daily","prompt":"run","invocation":{"kind":"Schedule"}}"#,
        r#"{"type":"TriggerStarted","trigger_id":"id","trigger_name":"sleep-import","invocation":{"kind":"Event","event_type":"DataImported","event_id":"00000000-0000-0000-0000-000000000001"}}"#,
        r#"{"type":"TriggerStarted","task_id":"id","task_name":"legacy"}"#,
        r#"{"type":"TriggerCompleted","trigger_id":"id"}"#,
        r#"{"type":"TriggerCompleted","trigger_id":"id","trigger_name":"daily","result_summary":"done"}"#,
        r#"{"type":"TriggerCompleted","task_id":"id","task_name":"legacy"}"#,
        // Changes — old format (path/diff)
        r#"{"type":"ChangeProposed","path":"p","diff":"d"}"#,
        r#"{"type":"ChangeApplied","path":"p"}"#,
        r#"{"type":"ChangeDiscarded","path":"p"}"#,
        r#"{"type":"ChangeReverted","path":"p"}"#,
        // Changes — new format (change_id)
        r#"{"type":"ChangeProposed","change_id":"c-1","description":"fix","files":["a.rs"],"requires_restart":true}"#,
        r#"{"type":"ChangeApplied","change_id":"c-1","requires_restart":false}"#,
        r#"{"type":"ChangeDiscarded","change_id":"c-1"}"#,
        r#"{"type":"ChangeReverted","change_id":"c-1"}"#,
        r#"{"type":"ChangeApplyFailed","change_id":"c-1","error":"merge conflict"}"#,
        // Interactive
        r#"{"type":"CredentialRequested","provider":"github"}"#,
        r#"{"type":"McpConsentRequested","tool":"t","args":{}}"#,
    ];
    for json_str in known_types {
        let result: Result<ThreadEvent, _> = serde_json::from_str(json_str);
        assert!(
            result.is_ok(),
            "Failed to deserialize: {}\nError: {:?}",
            json_str,
            result.err()
        );
    }
}

#[test]
fn to_payload_removes_type_tag() {
    let event = ThreadEvent::ToolCalled {
        name: "read_file".to_string(),
        args: json!({"path": "test.txt"}),
        description: "Reading test.txt...".to_string(),
    };
    let payload = event.to_payload(&EventMeta::NONE);
    assert!(
        payload.get("type").is_none(),
        "to_payload() must strip the 'type' tag"
    );
    assert_eq!(payload["name"], "read_file");
    assert_eq!(payload["args"]["path"], "test.txt");

    // ResponseGenerated with empty text — should produce empty object (skip_serializing_if)
    let event2 = ThreadEvent::ResponseGenerated {
        text: String::new(),
        images: vec![],
        model: None,
        reasoning_effort: None,
    };
    let payload2 = event2.to_payload(&EventMeta::NONE);
    assert!(payload2.get("type").is_none());
    assert!(
        payload2.as_object().unwrap().is_empty(),
        "empty ResponseGenerated should produce {{}}"
    );

    // ResponseGenerated with content
    let event3 = ThreadEvent::ResponseGenerated {
        text: "answer".into(),
        images: vec!["img.png".into()],
        model: None,
        reasoning_effort: None,
    };
    let payload3 = event3.to_payload(&EventMeta::NONE);
    assert_eq!(payload3["text"], "answer");
    assert_eq!(payload3["images"][0], "img.png");
}

#[test]
fn claude_code_idled_has_changes_serialization() {
    // With has_changes=true → field included
    let event = ThreadEvent::CodingAgentIdled {
        has_changes: true,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        agent: crate::runtime::AgentKind::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
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
        agent: crate::runtime::AgentKind::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
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
        agent: crate::runtime::AgentKind::ClaudeCode,
        tool_use_id: String::new(),
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["description"], "Read main.rs");

    // Empty → skipped
    let event2 = ThreadEvent::CodingAgentToolCalled {
        name: "Read".into(),
        args: json!({}),
        description: String::new(),
        agent: crate::runtime::AgentKind::ClaudeCode,
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
        agent: crate::runtime::AgentKind::ClaudeCode,
        tool_use_id: "toolu_42".into(),
    };
    let serialized = serde_json::to_value(&call).unwrap();
    assert_eq!(serialized["tool_use_id"], "toolu_42");

    // Empty id → skipped from the wire
    let call_no_id = ThreadEvent::CodingAgentToolCalled {
        name: "Bash".into(),
        args: json!({}),
        description: String::new(),
        agent: crate::runtime::AgentKind::ClaudeCode,
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
        }),
        origin: None,
        go_to_review: false,
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

#[test]
fn event_meta_defaults() {
    let meta = EventMeta::default();
    assert!(meta.request_event_id.is_none());
    assert!(meta.channel.is_none());
    assert!(meta.event_id.is_none());
}

#[test]
fn event_meta_merges_into_payload() {
    let event = ThreadEvent::ResponseGenerated {
        text: "answer".into(),
        images: vec![],
        model: None,
        reasoning_effort: None,
    };
    let meta = EventMeta {
        request_event_id: Some(
            uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap(),
        ),
        channel: Some(EventChannel::CodingAgent),
        event_id: Some(uuid::Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap()),
        actor: None,
    };
    let payload = event.to_payload(&meta);
    assert_eq!(payload["text"], "answer");
    assert_eq!(
        payload["request_event_id"],
        "12345678-1234-1234-1234-123456789abc"
    );
    assert_eq!(payload["channel"], "claude_code");
    // event_id is NOT merged into payload — it's used as the DB primary key
    assert!(payload.get("event_id").is_none());
}

#[test]
fn event_meta_none_adds_nothing() {
    let event = ThreadEvent::TextStreamed {
        text: "chunk".into(),
    };
    let payload = event.to_payload(&EventMeta::NONE);
    assert_eq!(payload["text"], "chunk");
    assert!(payload.get("request_event_id").is_none());
    assert!(payload.get("channel").is_none());
    assert!(payload.get("event_id").is_none());
}

#[test]
fn event_meta_actor_merges_into_payload() {
    // Auditability: every mutating endpoint stamps the event with who
    // initiated it. EventMeta carries that across all ThreadEvent variants
    // without per-variant struct churn or backward-compat churn for unit
    // variants like ThreadSaved.
    let event = ThreadEvent::ThreadSaved;
    let meta = EventMeta {
        actor: Some(MessageOrigin::Device {
            device_id: "dev-1".into(),
            label: "Chrome on Mac".into(),
        }),
        ..EventMeta::NONE
    };
    let payload = event.to_payload(&meta);
    assert_eq!(payload["actor"]["kind"], "device");
    assert_eq!(payload["actor"]["device_id"], "dev-1");
    assert_eq!(payload["actor"]["label"], "Chrome on Mac");
}

#[test]
fn event_meta_actor_none_omits_field() {
    let event = ThreadEvent::ThreadSaved;
    let payload = event.to_payload(&EventMeta::NONE);
    assert!(payload.get("actor").is_none());
}

#[test]
fn indexable_text_returns_content_for_chat_events() {
    let msg = ThreadEvent::MessageReceived {
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
    assert_eq!(msg.indexable_text(), Some("hello"));

    let resp = ThreadEvent::ResponseGenerated {
        text: "answer".into(),
        images: vec![],
        model: None,
        reasoning_effort: None,
    };
    assert_eq!(resp.indexable_text(), Some("answer"));

    let canceled = ThreadEvent::ResponseCanceled {
        text: "partial".into(),
        images: vec![],
        model: None,
        reasoning_effort: None,
        cause: crate::engine::thread_events::CancelCause::UserStop,
    };
    assert_eq!(canceled.indexable_text(), Some("partial"));
}

#[test]
fn indexable_text_returns_none_for_non_chat_events() {
    assert!(ThreadEvent::TextStreamed {
        text: "chunk".into()
    }
    .indexable_text()
    .is_none());
    assert!(ThreadEvent::ToolCalled {
        name: "x".into(),
        args: json!({}),
        description: String::new()
    }
    .indexable_text()
    .is_none());
    assert!(ThreadEvent::ToolResult {
        name: "x".into(),
        result: "ok".into(),
        images: vec![],
        success: true,
    }
    .indexable_text()
    .is_none());
    assert!(ThreadEvent::SessionStarted {
        session_id: "s".into(),
        branch: String::new(),
        repo_id: None
    }
    .indexable_text()
    .is_none());
    assert!(ThreadEvent::SessionEnded {
        reason: SessionEndReason::Shutdown
    }
    .indexable_text()
    .is_none());
    assert!(ThreadEvent::ThreadTitleGenerated { title: "t".into() }
        .indexable_text()
        .is_none());
}

#[test]
fn user_question_asked_serialization() {
    let event = ThreadEvent::UserQuestionAsked {
        tool_use_id: "tu_1".into(),
        cc_session_id: "sess_abc".into(),
        question: "Pick one:".into(),
        options: vec![
            QuestionOption {
                id: "o1".into(),
                label: "First".into(),
                description: Some("desc".into()),
            },
            QuestionOption {
                id: "o2".into(),
                label: "Second".into(),
                description: None,
            },
        ],
        worktree_path: Some("/tmp/cc-abc".into()),
        multi_select: false,
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["type"], "UserQuestionAsked");
    assert_eq!(v["tool_use_id"], "tu_1");
    assert_eq!(v["cc_session_id"], "sess_abc");
    assert_eq!(v["question"], "Pick one:");
    assert_eq!(v["options"][0]["id"], "o1");
    assert_eq!(v["options"][0]["label"], "First");
    assert_eq!(v["options"][0]["description"], "desc");
    assert_eq!(v["options"][1]["id"], "o2");
    assert!(
        v["options"][1].get("description").is_none(),
        "None description should be skipped"
    );
    assert_eq!(v["worktree_path"], "/tmp/cc-abc");
}

#[test]
fn user_question_asked_empty_options_skipped() {
    let event = ThreadEvent::UserQuestionAsked {
        tool_use_id: "tu_1".into(),
        cc_session_id: "sess_abc".into(),
        question: "Continue?".into(),
        options: vec![],
        worktree_path: None,
        multi_select: false,
    };
    let v = serde_json::to_value(&event).unwrap();
    assert!(
        v.get("options").is_none(),
        "empty options should be skipped"
    );
    assert!(
        v.get("worktree_path").is_none(),
        "None worktree_path should be skipped — keeps payload small for the common case"
    );
}

#[test]
fn user_question_asked_event_type() {
    let event = ThreadEvent::UserQuestionAsked {
        tool_use_id: "tu_1".into(),
        cc_session_id: "sess_abc".into(),
        question: "?".into(),
        options: vec![],
        worktree_path: None,
        multi_select: false,
    };
    assert_eq!(event.event_type(), "UserQuestionAsked");
    assert!(event.is_persisted(), "UserQuestionAsked must be persisted");
}

#[test]
fn user_question_asked_carries_multi_select() {
    let event = ThreadEvent::UserQuestionAsked {
        tool_use_id: "tu_1".into(),
        cc_session_id: "sess".into(),
        question: "Pick many:".into(),
        options: vec![],
        worktree_path: None,
        multi_select: true,
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["multi_select"], true);

    let legacy =
        r#"{"type":"UserQuestionAsked","tool_use_id":"tu","cc_session_id":"s","question":"q"}"#;
    let parsed: ThreadEvent = serde_json::from_str(legacy).expect("legacy parses");
    match parsed {
        ThreadEvent::UserQuestionAsked { multi_select, .. } => {
            assert!(!multi_select, "missing multi_select must default to false");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn user_question_answered_selected_serialization() {
    let event = ThreadEvent::UserQuestionAnswered {
        tool_use_id: "tu_1".into(),
        answer: AnswerKind::Selected {
            option_id: "o1".into(),
        },
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["type"], "UserQuestionAnswered");
    assert_eq!(v["tool_use_id"], "tu_1");
    assert_eq!(v["answer"]["kind"], "Selected");
    assert_eq!(v["answer"]["option_id"], "o1");
}

#[test]
fn user_question_answered_free_text_serialization() {
    let event = ThreadEvent::UserQuestionAnswered {
        tool_use_id: "tu_1".into(),
        answer: AnswerKind::FreeText {
            text: "let's do X".into(),
        },
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["answer"]["kind"], "FreeText");
    assert_eq!(v["answer"]["text"], "let's do X");
}

#[test]
fn user_question_answered_canceled_serialization() {
    let event = ThreadEvent::UserQuestionAnswered {
        tool_use_id: "tu_1".into(),
        answer: AnswerKind::Canceled,
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["answer"]["kind"], "Canceled");
}

#[test]
fn user_question_answered_multi_selected_serialization() {
    let event = ThreadEvent::UserQuestionAnswered {
        tool_use_id: "tu_1".into(),
        answer: AnswerKind::MultiSelected {
            option_ids: vec!["opt-0".into(), "opt-2".into()],
            text: None,
        },
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["type"], "UserQuestionAnswered");
    assert_eq!(v["answer"]["kind"], "MultiSelected");
    assert_eq!(v["answer"]["option_ids"][0], "opt-0");
    assert_eq!(v["answer"]["option_ids"][1], "opt-2");
    assert!(
        v["answer"].get("text").is_none(),
        "text omitted when None — keeps legacy payload shape"
    );

    let raw = r#"{"type":"UserQuestionAnswered","tool_use_id":"tu_1","answer":{"kind":"MultiSelected","option_ids":["opt-0","opt-2"]}}"#;
    let parsed: ThreadEvent = serde_json::from_str(raw).expect("parse");
    match parsed {
        ThreadEvent::UserQuestionAnswered {
            answer: AnswerKind::MultiSelected { option_ids, text },
            ..
        } => {
            assert_eq!(option_ids, vec!["opt-0", "opt-2"]);
            assert!(text.is_none(), "legacy payload deserializes text=None");
        }
        other => panic!("expected MultiSelected, got {:?}", other),
    }
}

#[test]
fn user_question_answered_multi_selected_with_text_serialization() {
    // New shape: MultiSelected carrying the prompt textarea's contents
    // alongside the toggled option ids. Round-trips with `text` present.
    let event = ThreadEvent::UserQuestionAnswered {
        tool_use_id: "tu_2".into(),
        answer: AnswerKind::MultiSelected {
            option_ids: vec!["opt-0".into()],
            text: Some("plus this".into()),
        },
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["answer"]["text"], "plus this");

    let raw = serde_json::to_string(&event).unwrap();
    let parsed: ThreadEvent = serde_json::from_str(&raw).expect("parse");
    match parsed {
        ThreadEvent::UserQuestionAnswered {
            answer: AnswerKind::MultiSelected { option_ids, text },
            ..
        } => {
            assert_eq!(option_ids, vec!["opt-0"]);
            assert_eq!(text.as_deref(), Some("plus this"));
        }
        other => panic!("expected MultiSelected, got {:?}", other),
    }
}

#[test]
fn user_question_answered_event_type() {
    let event = ThreadEvent::UserQuestionAnswered {
        tool_use_id: "tu_1".into(),
        answer: AnswerKind::Canceled,
    };
    assert_eq!(event.event_type(), "UserQuestionAnswered");
    assert!(
        event.is_persisted(),
        "UserQuestionAnswered must be persisted"
    );
}

#[test]
fn user_question_round_trips_through_db_payload() {
    // Old DB rows or fresh inserts must deserialize cleanly.
    let cases = [
        r#"{"type":"UserQuestionAsked","tool_use_id":"tu","cc_session_id":"s","question":"q"}"#,
        r#"{"type":"UserQuestionAsked","tool_use_id":"tu","cc_session_id":"s","question":"q","options":[{"id":"a","label":"A"}]}"#,
        r#"{"type":"UserQuestionAnswered","tool_use_id":"tu","answer":{"kind":"Selected","option_id":"a"}}"#,
        r#"{"type":"UserQuestionAnswered","tool_use_id":"tu","answer":{"kind":"FreeText","text":"hi"}}"#,
        r#"{"type":"UserQuestionAnswered","tool_use_id":"tu","answer":{"kind":"Canceled"}}"#,
        r#"{"type":"UserQuestionAnswered","tool_use_id":"tu","answer":{"kind":"MultiSelected","option_ids":["a","b"]}}"#,
    ];
    for raw in cases {
        let parsed: Result<ThreadEvent, _> = serde_json::from_str(raw);
        assert!(
            parsed.is_ok(),
            "Failed to deserialize {}: {:?}",
            raw,
            parsed.err()
        );
    }
}

/// Pin the contract that legacy DB rows without `cause` deserialize as
/// `Unknown` rather than failing, and that fresh emissions round-trip
/// the typed cause through serde.
#[test]
fn response_cancel_abort_cause_round_trip_and_legacy_default() {
    // Legacy: no `cause` field on the wire → deserializes as Unknown.
    let legacy_canceled: ThreadEvent =
        serde_json::from_str(r#"{"type":"ResponseCanceled","text":"x"}"#).unwrap();
    match legacy_canceled {
        ThreadEvent::ResponseCanceled { cause, .. } => {
            assert_eq!(
                cause,
                CancelCause::Unknown,
                "legacy rows default to Unknown"
            )
        }
        _ => panic!("expected ResponseCanceled"),
    }
    let legacy_aborted: ThreadEvent =
        serde_json::from_str(r#"{"type":"ResponseAborted"}"#).unwrap();
    match legacy_aborted {
        ThreadEvent::ResponseAborted { cause, .. } => {
            assert_eq!(cause, AbortCause::Unknown, "legacy rows default to Unknown")
        }
        _ => panic!("expected ResponseAborted"),
    }

    // Removed cause string: `stale_settle` was a CancelCause variant in earlier
    // builds before being moved to AbortCause. Old DB rows persisted while it
    // was a cancel cause must replay cleanly via `#[serde(other)] Unknown`,
    // not crash deserialization.
    let removed_cancel_cause: ThreadEvent =
        serde_json::from_str(r#"{"type":"ResponseCanceled","cause":"stale_settle"}"#).unwrap();
    match removed_cancel_cause {
        ThreadEvent::ResponseCanceled { cause, .. } => {
            assert_eq!(
                cause,
                CancelCause::Unknown,
                "removed cause strings must fall back to Unknown via #[serde(other)]"
            )
        }
        _ => panic!("expected ResponseCanceled"),
    }

    // Fresh emit with typed cause survives the serde round trip in both
    // directions — the wire format uses snake_case strings.
    for cancel_cause in [
        CancelCause::UserStop,
        CancelCause::UserAction,
        CancelCause::Unknown,
    ] {
        let event = ThreadEvent::ResponseCanceled {
            text: "p".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: cancel_cause,
        };
        let json = serde_json::to_value(&event).unwrap();
        let round: ThreadEvent = serde_json::from_value(json.clone()).unwrap();
        match round {
            ThreadEvent::ResponseCanceled { cause, .. } => assert_eq!(cause, cancel_cause),
            _ => panic!("wrong variant"),
        }
    }
    for abort_cause in [
        AbortCause::EngineShutdown,
        AbortCause::SafetyNet,
        AbortCause::RecoveryAfterRestart,
        AbortCause::ProcessKilled,
        AbortCause::StaleSettle,
        AbortCause::Unknown,
    ] {
        let event = ThreadEvent::ResponseAborted {
            text: "p".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: abort_cause,
        };
        let json = serde_json::to_value(&event).unwrap();
        let round: ThreadEvent = serde_json::from_value(json.clone()).unwrap();
        match round {
            ThreadEvent::ResponseAborted { cause, .. } => assert_eq!(cause, abort_cause),
            _ => panic!("wrong variant"),
        }
    }
}

#[test]
fn session_ended_reason_serialization() {
    // Each emit-able variant round-trips on the wire.
    for (reason, expected) in [
        (SessionEndReason::Shutdown, "shutdown"),
        (SessionEndReason::Panic, "panic"),
        (SessionEndReason::Closed, "closed"),
        (SessionEndReason::StaleResume, "stale_resume"),
    ] {
        let event = ThreadEvent::SessionEnded { reason };
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized["type"], "SessionEnded");
        assert_eq!(
            serialized["reason"], expected,
            "{:?} must serialize as {:?}",
            reason, expected
        );
    }

    // Backwards compat: old DB rows without a `reason` field deserialize
    // as `LegacyNonTerminal` via the serde default.
    let old: ThreadEvent = serde_json::from_str(r#"{"type":"SessionEnded"}"#).unwrap();
    match old {
        ThreadEvent::SessionEnded { reason } => {
            assert_eq!(reason, SessionEndReason::LegacyNonTerminal)
        }
        _ => panic!("wrong variant"),
    }

    // Backwards compat: removed reasons (completed, changes_proposed,
    // changes_applied, auto_ended, user_ended, discarded) on legacy rows
    // deserialize via `#[serde(other)]` to `LegacyNonTerminal` so old data
    // doesn't crash the engine.
    for legacy in [
        "completed",
        "user_ended",
        "changes_proposed",
        "changes_applied",
        "auto_ended",
        "discarded",
    ] {
        let raw = format!(r#"{{"type":"SessionEnded","reason":"{}"}}"#, legacy);
        let parsed: ThreadEvent = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("legacy reason {:?} should deserialize: {}", legacy, e));
        match parsed {
            ThreadEvent::SessionEnded { reason } => assert_eq!(
                reason,
                SessionEndReason::LegacyNonTerminal,
                "legacy reason {:?} should map to LegacyNonTerminal",
                legacy
            ),
            _ => panic!("wrong variant for legacy reason {:?}", legacy),
        }
    }
}

#[test]
fn continuation_started_event_can_carry_engine_origin() {
    let event = ThreadEvent::ContinuationStarted {
        branch: "claude-code/20260422".into(),
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::ContinuationStarted,
        }),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ContinuationStarted");
    assert_eq!(json["origin"]["kind"], "engine");
    assert_eq!(json["origin"]["reason"]["kind"], "continuation_started");
}

#[test]
fn continuation_started_event_origin_defaults_to_none_when_missing() {
    // Old DB rows without origin must deserialize cleanly.
    let json = r#"{"type":"ContinuationStarted","branch":"claude-code/20260318"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::ContinuationStarted { branch, origin } => {
            assert_eq!(branch, "claude-code/20260318");
            assert!(origin.is_none());
        }
        other => panic!("expected ContinuationStarted, got {:?}", other),
    }
}

#[test]
fn continuation_started_event_accepts_legacy_session_recovered_alias() {
    // Old DB rows persisted as SessionRecovered must still deserialize as the
    // renamed ContinuationStarted variant. Also covers the older SessionResumed
    // name (renamed → SessionRecovered in 2026-03-20, then →
    // ContinuationStarted in 2026-05-13). Without the serde aliases the
    // projection blows up on every historical row.
    for legacy in &[
        r#"{"type":"SessionRecovered","branch":"claude-code/legacy"}"#,
        r#"{"type":"SessionResumed","branch":"claude-code/older-legacy"}"#,
    ] {
        let parsed: ThreadEvent = serde_json::from_str(legacy)
            .unwrap_or_else(|e| panic!("legacy {} should deserialize: {}", legacy, e));
        match parsed {
            ThreadEvent::ContinuationStarted { .. } => {}
            other => panic!("expected ContinuationStarted from {}, got {:?}", legacy, other),
        }
    }
}

#[test]
fn change_proposed_event_can_carry_engine_origin() {
    let event = ThreadEvent::ChangeProposed {
        change_id: "abc".into(),
        description: Some("stale session cleanup".into()),
        files: vec!["src/main.rs".into()],
        requires_restart: false,
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::StaleSession,
        }),
        commit_sha: None,
        branch_name: String::new(),
        repo_root: String::new(),
        hardened: false,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ChangeProposed");
    assert_eq!(json["origin"]["kind"], "engine");
    assert_eq!(json["origin"]["reason"]["kind"], "stale_session");
}

#[test]
fn change_proposed_event_origin_defaults_to_none_when_missing() {
    // Old DB rows without origin must deserialize cleanly.
    let json = r#"{"type":"ChangeProposed","change_id":"x","description":"y","files":[],"requires_restart":false}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::ChangeProposed {
            change_id, origin, ..
        } => {
            assert_eq!(change_id, "x");
            assert!(origin.is_none());
        }
        other => panic!("expected ChangeProposed, got {:?}", other),
    }
}

#[test]
fn change_proposed_event_can_carry_orphan_recovery_origin() {
    let event = ThreadEvent::ChangeProposed {
        change_id: "def".into(),
        description: Some("orphan cleanup".into()),
        files: vec!["src/lib.rs".into()],
        requires_restart: false,
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::OrphanRecovery,
        }),
        commit_sha: None,
        branch_name: String::new(),
        repo_root: String::new(),
        hardened: false,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["origin"]["reason"]["kind"], "orphan_recovery");
}

#[test]
fn coding_agent_prompt_sent_can_carry_orphan_recovery_origin() {
    let event = ThreadEvent::CodingAgentPromptSent {
        text: "resume after restart".into(),
        agent: crate::runtime::AgentKind::ClaudeCode,
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::OrphanRecovery,
        }),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "CodingAgentPromptSent");
    assert_eq!(json["origin"]["kind"], "engine");
    assert_eq!(json["origin"]["reason"]["kind"], "orphan_recovery");
}

#[test]
fn coding_agent_prompt_sent_can_carry_harden_retrigger_origin() {
    let event = ThreadEvent::CodingAgentPromptSent {
        text: "Run /harden now.".into(),
        agent: crate::runtime::AgentKind::ClaudeCode,
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::HardenRetrigger,
        }),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "CodingAgentPromptSent");
    assert_eq!(json["origin"]["kind"], "engine");
    assert_eq!(json["origin"]["reason"]["kind"], "harden_retrigger");
}

#[test]
fn coding_agent_prompt_sent_origin_defaults_to_none_when_missing() {
    let json = r#"{"type":"CodingAgentPromptSent","text":"hi"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::CodingAgentPromptSent { text, origin, .. } => {
            assert_eq!(text, "hi");
            assert!(origin.is_none());
        }
        other => panic!("expected CodingAgentPromptSent, got {:?}", other),
    }
}

#[test]
fn trigger_started_can_carry_scheduler_origin() {
    let id = uuid::Uuid::new_v4().to_string();
    let event = ThreadEvent::TriggerStarted {
        trigger_id: id.clone(),
        trigger_name: Some("nightly".into()),
        prompt: Some("run".into()),
        invocation: Some(TriggerInvocation::Schedule),
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::Scheduler {
                trigger_id: id.clone(),
                trigger_name: Some("nightly".into()),
            },
        }),
        go_to_review: false,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "TriggerStarted");
    assert_eq!(json["origin"]["reason"]["kind"], "scheduler");
    assert_eq!(json["origin"]["reason"]["trigger_id"], id);
    assert_eq!(json["origin"]["reason"]["trigger_name"], "nightly");
}

#[test]
fn trigger_started_origin_defaults_to_none_when_missing() {
    let json = r#"{"type":"TriggerStarted","trigger_id":"id"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::TriggerStarted { origin, .. } => assert!(origin.is_none()),
        other => panic!("expected TriggerStarted, got {:?}", other),
    }
}

// ---- `mode` field deserialization for MessageReceived ----

#[test]
fn message_received_mode_field_deserializes() {
    let json = r#"{"type":"MessageReceived","text":"hi","mode":"engine"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::MessageReceived { mode, .. } => assert_eq!(mode, ActorMode::Engine),
        other => panic!("expected MessageReceived, got {:?}", other),
    }
}

/// Legacy DB rows predating the `mode` field must replay as `Human` so
/// historical events keep loading. New emissions are forced by the API
/// layer to set `mode` explicitly.
#[test]
fn message_received_no_mode_defaults_to_human() {
    let json = r#"{"type":"MessageReceived","text":"hi"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::MessageReceived { mode, .. } => assert_eq!(mode, ActorMode::Human),
        other => panic!("expected MessageReceived, got {:?}", other),
    }
}

/// ImageUploaded is a per-thread audit fact emitted by POST /threads/:id/blobs.
/// The hash uniquely identifies the blob bytes (sha256 hex, 64 chars). The
/// mime + byte_size are convenience fields so consumers can render the
/// upload entry without a HEAD on the blob endpoint. Past-tense, persisted.
#[test]
fn image_uploaded_serializes_with_all_fields() {
    let event = ThreadEvent::ImageUploaded {
        hash: "a".repeat(64),
        mime: "image/png".to_string(),
        byte_size: 4096,
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ImageUploaded");
    assert_eq!(json["hash"], "a".repeat(64));
    assert_eq!(json["mime"], "image/png");
    assert_eq!(json["byte_size"], 4096);
    // actor: None must skip-serialize so the wire shape matches the
    // pattern used by ThreadStarted / ThreadDiscarded — frontend treats
    // missing actor as "unknown", not as a literal null.
    assert!(json.get("actor").is_none());
}

/// ImageUploaded is reported by `event_type()` so the projection / SSE
/// dispatcher can route by name without matching on the variant. The
/// name must match the PascalCase variant exactly (used in JSONB queries).
#[test]
fn image_uploaded_event_type_is_pascal_case_name() {
    let event = ThreadEvent::ImageUploaded {
        hash: "b".repeat(64),
        mime: "image/jpeg".to_string(),
        byte_size: 1,
        actor: None,
    };
    assert_eq!(event.event_type(), "ImageUploaded");
}

/// ImageUploaded is past-tense and represents a durable fact (the user
/// attached this image). `is_persisted()` must agree so the EventBus
/// writes a row to the events table — without persistence the audit
/// trail and migration story collapse.
#[test]
fn image_uploaded_is_persisted() {
    let event = ThreadEvent::ImageUploaded {
        hash: "c".repeat(64),
        mime: "image/webp".to_string(),
        byte_size: 1,
        actor: None,
    };
    assert!(event.is_persisted());
}

/// ContextCaptured is the unified replacement for Thinking-with-tokens +
/// ContextTokensMeasured + ContextAssembled. One event per LLM call carries
/// the full picture: producer, model + budget, per-section breakdown
/// (system, tools, history, …), an estimated total, and — once the API
/// responds — real token usage including cache hit/miss. The frontend
/// renders a single modal from this; old events are projected to the same
/// shape at read time. New fields are `Option`/`#[serde(default)]` so DB
/// rows persisted before usage existed deserialize cleanly.
#[test]
fn context_captured_event_type_and_persistence() {
    let event = ThreadEvent::ContextCaptured {
        producer: crate::engine::ContextProducer::MainLlm,
        model: "claude-opus-4-7".to_string(),
        context_window: 1_000_000,
        sections: vec![crate::engine::ContextSection {
            name: "User Message".to_string(),
            content: Some("hi".to_string()),
            char_count: 2,
        }],
        tools: vec!["read_file".to_string()],
        estimated_total_tokens: 1,
        usage: Some(crate::engine::ApiUsage {
            input_tokens: 12_345,
            output_tokens: 678,
            cache_read_tokens: 10_000,
            cache_creation_tokens: 200,
        }),
        trimmed: false,
    };
    assert_eq!(event.event_type(), "ContextCaptured");
    assert!(event.is_persisted());

    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["type"], "ContextCaptured");
    assert_eq!(serialized["producer"], "main_llm");
    assert_eq!(serialized["model"], "claude-opus-4-7");
    assert_eq!(serialized["context_window"], 1_000_000);
    assert_eq!(serialized["sections"][0]["name"], "User Message");
    assert_eq!(serialized["sections"][0]["content"], "hi");
    assert_eq!(serialized["tools"][0], "read_file");
    assert_eq!(serialized["estimated_total_tokens"], 1);
    assert_eq!(serialized["usage"]["input_tokens"], 12_345);
    assert_eq!(serialized["usage"]["output_tokens"], 678);
    assert_eq!(serialized["usage"]["cache_read_tokens"], 10_000);
    assert_eq!(serialized["usage"]["cache_creation_tokens"], 200);

    // Pre-call capture: usage absent, content omitted (capture_context off).
    let pre_call_json = r#"{
            "type":"ContextCaptured",
            "producer":"claude_code",
            "model":"claude-sonnet-4-6",
            "context_window":200000,
            "sections":[{"name":"Conversation","char_count":500}],
            "tools":[],
            "estimated_total_tokens":125,
            "trimmed":false
        }"#;
    let parsed: ThreadEvent = serde_json::from_str(pre_call_json).unwrap();
    match parsed {
        ThreadEvent::ContextCaptured {
            producer,
            usage,
            sections,
            ..
        } => {
            assert_eq!(producer, crate::engine::ContextProducer::ClaudeCode);
            assert!(usage.is_none(), "usage should be None pre-call");
            assert!(
                sections[0].content.is_none(),
                "content should be None when capture_context off"
            );
        }
        other => panic!("expected ContextCaptured, got {other:?}"),
    }
}

#[test]
fn tool_result_serializes_success_field() {
    let evt = ThreadEvent::ToolResult {
        name: "x".into(),
        result: "ok".into(),
        images: vec![],
        success: true,
    };
    // ThreadEvent uses #[serde(tag = "type")] (internally tagged), so
    // payload fields land at the top level alongside the `type` discriminator.
    let json = serde_json::to_value(&evt).unwrap();
    assert_eq!(json["type"], "ToolResult");
    assert_eq!(json["success"], serde_json::json!(true));
}

#[test]
fn tool_result_round_trips_success_false() {
    let evt = ThreadEvent::ToolResult {
        name: "x".into(),
        result: "Error: nope".into(),
        images: vec![],
        success: false,
    };
    let round: ThreadEvent = serde_json::from_value(serde_json::to_value(&evt).unwrap()).unwrap();
    match round {
        ThreadEvent::ToolResult { success, .. } => assert!(!success),
        _ => panic!("wrong variant"),
    }
}

/// `ChildThreadCompleted` is the typed replacement for the prose
/// `[Child thread completed] ...` user-message callback. The payload
/// must round-trip through serde so the event survives DB persistence and
/// SSE rebroadcast — the projection layer reads it without translation.
#[test]
fn child_thread_completed_event_round_trips() {
    let evt = ThreadEvent::ChildThreadCompleted {
        child_thread_id: uuid::Uuid::new_v4(),
        child_thread_title: Some("Nightly Step 1: Build & Test".into()),
        status: ChildCompletionStatus::Success,
        summary: "All green".into(),
        pending_change_ids: vec!["change-1".into()],
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert_eq!(v["type"], "ChildThreadCompleted");
    let back: ThreadEvent = serde_json::from_value(v).unwrap();
    assert!(matches!(back, ThreadEvent::ChildThreadCompleted { .. }));
}

/// `event_type()` for `ChildThreadCompleted` is the PascalCase variant
/// name — the projection / SSE dispatcher matches by name, so a typo here
/// would silently break every consumer.
#[test]
fn child_thread_completed_event_type_is_pascal_case_name() {
    let evt = ThreadEvent::ChildThreadCompleted {
        child_thread_id: uuid::Uuid::new_v4(),
        child_thread_title: None,
        status: ChildCompletionStatus::NoChanges,
        summary: String::new(),
        pending_change_ids: vec![],
    };
    assert_eq!(evt.event_type(), "ChildThreadCompleted");
    assert!(evt.is_persisted());
}

/// Optional fields on the wire stay optional — `child_thread_title` and
/// `pending_change_ids` are skip-serialized when empty so SSE consumers
/// don't see literal nulls / empty arrays cluttering the payload.
#[test]
fn child_thread_completed_skips_empty_optional_fields() {
    let evt = ThreadEvent::ChildThreadCompleted {
        child_thread_id: uuid::Uuid::new_v4(),
        child_thread_title: None,
        status: ChildCompletionStatus::Failure,
        summary: "boom".into(),
        pending_change_ids: vec![],
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert!(v.get("child_thread_title").is_none());
    assert!(v.get("pending_change_ids").is_none());
    assert_eq!(v["status"], "failure");
}

/// `ChildCompletionStatus` is serialized snake_case — the wire constants
/// the frontend reads are `success` / `failure` / `no_changes`. The plan's
/// status mapping (`CodingAgentIdled has_changes:true → success`,
/// `has_changes:false → no_changes`, `ResponseFailed → failure`) depends
/// on these exact strings.
#[test]
fn child_completion_status_serializes_snake_case() {
    assert_eq!(
        serde_json::to_value(ChildCompletionStatus::Success).unwrap(),
        serde_json::json!("success")
    );
    assert_eq!(
        serde_json::to_value(ChildCompletionStatus::Failure).unwrap(),
        serde_json::json!("failure")
    );
    assert_eq!(
        serde_json::to_value(ChildCompletionStatus::NoChanges).unwrap(),
        serde_json::json!("no_changes")
    );
}

/// `indexable_text()` surfaces the summary so memory indexing /
/// auto-search can pick up child-completion notes alongside chat content.
#[test]
fn child_thread_completed_indexable_text_returns_summary() {
    let evt = ThreadEvent::ChildThreadCompleted {
        child_thread_id: uuid::Uuid::new_v4(),
        child_thread_title: Some("Title".into()),
        status: ChildCompletionStatus::Success,
        summary: "deployment finished cleanly".into(),
        pending_change_ids: vec![],
    };
    assert_eq!(evt.indexable_text(), Some("deployment finished cleanly"));
}

/// `ContextDismissed` is emitted by the `dismiss_from_context` tool. It
/// must round-trip and report its PascalCase event_type — the resume
/// helper queries the events table by event_type to collect dismissals.
#[test]
fn context_dismissed_event_round_trips() {
    let id = uuid::Uuid::new_v4();
    let evt = ThreadEvent::ContextDismissed {
        dismissed_event_id: id,
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert_eq!(v["type"], "ContextDismissed");
    assert_eq!(v["dismissed_event_id"], id.to_string());
    let back: ThreadEvent = serde_json::from_value(v).unwrap();
    assert!(matches!(back, ThreadEvent::ContextDismissed { .. }));
    assert_eq!(evt.event_type(), "ContextDismissed");
    assert!(evt.is_persisted());
}
