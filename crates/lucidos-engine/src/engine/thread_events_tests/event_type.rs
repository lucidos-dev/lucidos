use super::*;
use serde_json::json;

/// Drift guard: `RESERVED_TYPE_NAMES` must list every variant.
///
/// The truth comes from serde, which defines the wire names, rather than from a
/// hand-written sample of 85 constructors that can itself go stale. Feed an
/// internally tagged enum an unknown tag and it names every variant it would
/// have accepted. That list is what the const has to cover.
///
/// A miss means `emit_event` would let an app write a permanent domain row
/// under that thread-event name.
#[test]
fn reserved_type_names_cover_every_variant() {
    let err = serde_json::from_value::<ThreadEvent>(json!({ "type": "NoSuchVariant" }))
        .expect_err("an unknown tag must not deserialize")
        .to_string();
    let (_, listed) = err
        .split_once("expected one of ")
        .expect("serde must name the variants it would have accepted");
    let variants: Vec<&str> = listed
        .split(", ")
        .map(|s| s.trim().trim_matches('`'))
        .filter(|s| !s.is_empty())
        .collect();

    assert!(
        variants.len() > 50,
        "recovered only {} variants, so the parse is wrong, not the const: {err}",
        variants.len()
    );
    for name in &variants {
        assert!(
            ThreadEvent::is_reserved_type_name(name),
            "ThreadEvent::{name} is missing from RESERVED_TYPE_NAMES, so emit_event \
             would let an app write a domain row under that name"
        );
    }
    for name in ThreadEvent::RESERVED_TYPE_NAMES
        .iter()
        .chain(ThreadEvent::LEGACY_TYPE_NAME_ALIASES)
    {
        assert!(
            variants.contains(name),
            "the deny list carries {name}, which serde no longer accepts. A name \
             serde has dropped is dead weight, and a stale list hides a real one"
        );
    }
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
                voice_session_id: None,
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
        (
            ThreadEvent::ThoughtStreamed { text: "hmm".into() },
            "ThoughtStreamed",
        ),
        (
            ThreadEvent::MemoryRecalled {
                results: 5,
                queries: vec!["birthday".into()],
            },
            "MemoryRecalled",
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
                tool_called_event_id: None,
            },
            "ToolResult",
        ),
        (
            ThreadEvent::TodoListWritten {
                items: vec![],
                notes: None,
            },
            "TodoListWritten",
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
                reason: None,
            },
            "ContinuationStarted",
        ),
        (
            ThreadEvent::SessionStarted {
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: "s".into(),
                branch: String::new(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
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
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            },
            "CodingAgentTextStreamed",
        ),
        (
            ThreadEvent::CodingAgentToolCalled {
                name: "n".into(),
                args: json!({}),
                description: String::new(),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                tool_use_id: String::new(),
            },
            "CodingAgentToolCalled",
        ),
        (
            ThreadEvent::CodingAgentToolResult {
                name: "n".into(),
                result: "r".into(),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                tool_use_id: String::new(),
            },
            "CodingAgentToolResult",
        ),
        (
            ThreadEvent::CodingAgentUserMessageSent {
                text: "t".into(),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
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
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
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
                model: None,
                reasoning_effort: None,
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
    let event = ThreadEvent::LlmCallRetried {
        reason: "rate limited".to_string(),
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["type"], "LlmCallRetried");
    assert_eq!(serialized["reason"], "rate limited");

    let event2 = ThreadEvent::PreambleCompleted;
    let serialized2 = serde_json::to_value(&event2).unwrap();
    assert_eq!(serialized2["type"], "PreambleCompleted");

    let event3 = ThreadEvent::CodingAgentDiffChanged { has_diff: true };
    let serialized3 = serde_json::to_value(&event3).unwrap();
    assert_eq!(serialized3["type"], "CodingAgentDiffChanged");
    assert_eq!(serialized3["has_diff"], true);
    assert!(
        !event3.is_persisted(),
        "CodingAgentDiffChanged is an SSE-only aggregate refresh"
    );
}

/// Legacy persisted-row replay: any row written under a name we have since
/// renamed must still deserialize via the `#[serde(alias)]` declarations on the
/// new variant names, and must report the CANONICAL name from `event_type()` so
/// every downstream list keyed on that string keeps matching. Most of these
/// come from the T7 rename batch; `MemorySearched` is the 2026-08-12 split of
/// the automatic pre-turn recall away from the `memory` tool's own search.
#[test]
fn legacy_event_names_deserialize_via_alias() {
    let cases: &[(&str, &str)] = &[
        (r#"{"type":"Thinking","text":"hmm"}"#, "ThoughtStreamed"),
        (
            r#"{"type":"MemorySearched","results":3,"queries":["birthday"]}"#,
            "MemoryRecalled",
        ),
        (r#"{"type":"Retrying","reason":"r"}"#, "LlmCallRetried"),
        (
            r#"{"type":"TextStreaming","text":"t"}"#,
            "CumulativeTextUpdated",
        ),
        (r#"{"type":"PreambleCompleting"}"#, "PreambleCompleted"),
        (
            r#"{"type":"RefreshAppUI","app_id":"a"}"#,
            "AppUiRefreshRequested",
        ),
        (
            r#"{"type":"CaptureAppUI","app_id":"a","request_id":"r"}"#,
            "AppUiCaptureRequested",
        ),
        (
            r#"{"type":"CredentialRequest","payload":"{}"}"#,
            "CredentialPromptRequested",
        ),
        (
            r#"{"type":"PluginInstallRequest","payload":"{}"}"#,
            "PluginInstallRequested",
        ),
        (
            r#"{"type":"PluginUninstallRequest","payload":"{}"}"#,
            "PluginUninstallRequested",
        ),
        (
            r#"{"type":"EmailConfirmRequest","payload":"{}"}"#,
            "EmailConfirmRequested",
        ),
        (
            r#"{"type":"PushNotificationRequest"}"#,
            "PushNotificationRequested",
        ),
    ];
    for (json, expected_type) in cases {
        let event: ThreadEvent = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("legacy {json} failed to deserialize: {e}"));
        assert_eq!(
            event.event_type(),
            *expected_type,
            "legacy {} should map to {}",
            json,
            expected_type,
        );
    }
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
        r#"{"type":"MemoryRecalled","results":3}"#,
        r#"{"type":"MemoryRecalled","results":5,"queries":["birthday","date of birth"]}"#,
        r#"{"type":"ToolCalled","name":"x","args":{}}"#,
        r#"{"type":"ToolResult","name":"x","result":"ok"}"#,
        // TodoListWritten — empty + full list, all three statuses
        r#"{"type":"TodoListWritten","items":[]}"#,
        r#"{"type":"TodoListWritten","items":[{"content":"Run tests","active_form":"Running tests","status":"in_progress"}]}"#,
        r#"{"type":"TodoListWritten","items":[{"content":"a","active_form":"doing a","status":"pending"},{"content":"b","active_form":"doing b","status":"completed"}]}"#,
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
