use super::*;

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
            role: crate::engine::ContextRole::User,
            group: None,
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
        tool_called_event_id: None,
    };
    // ThreadEvent uses #[serde(tag = "type")] (internally tagged), so
    // payload fields land at the top level alongside the `type` discriminator.
    let json = serde_json::to_value(&evt).unwrap();
    assert_eq!(json["type"], "ToolResult");
    assert_eq!(json["success"], serde_json::json!(true));
    // Live emits don't carry tool_called_event_id — the skip_serializing_if
    // contract keeps it off the wire entirely so legacy clients (or the
    // resume builder reading the payload) see the same shape they always have.
    assert!(json.get("tool_called_event_id").is_none());
}

#[test]
fn tool_result_round_trips_success_false() {
    let evt = ThreadEvent::ToolResult {
        name: "x".into(),
        result: "Error: nope".into(),
        images: vec![],
        success: false,
        tool_called_event_id: None,
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

#[test]
fn image_described_event_round_trips() {
    let source = uuid::Uuid::new_v4();
    let evt = ThreadEvent::ImageDescribed {
        source_event_id: source,
        hash: "abcd1234".into(),
        description: "A cat sitting on a chair".into(),
        model: "claude-haiku-4-5".into(),
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert_eq!(v["type"], "ImageDescribed");
    assert_eq!(v["source_event_id"], source.to_string());
    assert_eq!(v["hash"], "abcd1234");
    assert_eq!(v["description"], "A cat sitting on a chair");
    assert_eq!(v["model"], "claude-haiku-4-5");
    let back: ThreadEvent = serde_json::from_value(v).unwrap();
    match back {
        ThreadEvent::ImageDescribed {
            source_event_id,
            hash,
            description,
            model,
        } => {
            assert_eq!(source_event_id, source);
            assert_eq!(hash, "abcd1234");
            assert_eq!(description, "A cat sitting on a chair");
            assert_eq!(model, "claude-haiku-4-5");
        }
        _ => panic!("wrong variant"),
    }
    assert_eq!(evt.event_type(), "ImageDescribed");
    assert!(
        evt.is_persisted(),
        "ImageDescribed is past-tense — must be persisted"
    );
    // The description carries real shared content, so it IS indexed into
    // memory (see indexable_text_includes_image_description).
    assert_eq!(evt.indexable_text(), Some("A cat sitting on a chair"));
}

/// Payload-shape contract: the wire layout is the variant's JSON object after
/// stripping the `type` discriminator. Mirrors `ChangeProposed`'s "to_payload
/// strips type" behavior — frontend reads `event_type` from the column and
/// the rest from `payload`.
#[test]
fn image_described_to_payload_strips_type_tag() {
    let source = uuid::Uuid::new_v4();
    let evt = ThreadEvent::ImageDescribed {
        source_event_id: source,
        hash: "deadbeef".into(),
        description: "screenshot".into(),
        model: "gemini-2.5-flash".into(),
    };
    let payload = evt.to_payload(&EventMeta::NONE);
    assert!(
        payload.get("type").is_none(),
        "to_payload must strip the discriminator"
    );
    assert_eq!(payload["source_event_id"], source.to_string());
    assert_eq!(payload["hash"], "deadbeef");
    assert_eq!(payload["description"], "screenshot");
    assert_eq!(payload["model"], "gemini-2.5-flash");
}

/// `is_per_token_streaming()` is the scheduler's blocklist gate — pin the
/// three variants it must return `true` for.
#[test]
fn is_per_token_streaming_blocks_text_chunk_variants() {
    assert!(ThreadEvent::TextStreamed {
        text: String::new(),
    }
    .is_per_token_streaming());
    assert!(ThreadEvent::ThoughtStreamed {
        text: String::new(),
    }
    .is_per_token_streaming());
    assert!(ThreadEvent::CodingAgentTextStreamed {
        text: String::new(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
    }
    .is_per_token_streaming());
}

/// Regression for the old allowlist: the four blocking-request events and the
/// common per-action / lifecycle / one-per-turn variants must NOT be
/// classified as per-token streaming, so the scheduler forwards them.
#[test]
fn is_per_token_streaming_allows_per_action_lifecycle_and_blocking_request_variants() {
    // Blocking-request events the old allowlist silently dropped.
    assert!(!ThreadEvent::UserQuestionAsked {
        tool_use_id: String::new(),
        cc_session_id: String::new(),
        question: String::new(),
        options: Vec::new(),
        worktree_path: None,
        multi_select: false,
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::CodingAgentPermissionRequest {
        request_id: String::new(),
        tool_use_id: String::new(),
        tool_name: String::new(),
        input: serde_json::Value::Null,
        summary: String::new(),
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::CredentialRequested {
        provider: String::new(),
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::McpConsentRequested {
        tool: String::new(),
        args: serde_json::Value::Null,
    }
    .is_per_token_streaming());
    // Per-action variants the matcher should see (workspaces scope with
    // `condition:` filters).
    assert!(!ThreadEvent::ToolCalled {
        name: String::new(),
        args: serde_json::Value::Null,
        description: String::new(),
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::CodingAgentToolCalled {
        name: String::new(),
        args: serde_json::Value::Null,
        description: String::new(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        tool_use_id: String::new(),
    }
    .is_per_token_streaming());
    // Lifecycle / one-per-turn variants.
    assert!(!ThreadEvent::ResponseGenerated {
        text: String::new(),
        images: Vec::new(),
        model: None,
        reasoning_effort: None,
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::CodingAgentIdled {
        has_changes: false,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::ThreadArchived.is_per_token_streaming());
}

// ──────────────────────────────────────────────────────────────────────────
// TodoListWritten — typed event carrying the Lucidos Agent's current todo
// list. Replace-whole-list semantics: each emission carries the full new
// state, so the projection / frontend just takes the latest.
// ──────────────────────────────────────────────────────────────────────────
