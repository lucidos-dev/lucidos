use super::*;

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
    assert!(json.get("actor").is_none());
}

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

#[test]
fn form_requests_and_their_resolution_are_persisted() {
    use crate::engine::thread_events::FormRequestOutcome;
    let request_id = uuid::Uuid::nil();
    let payload = || "{}".to_string();
    for event in [
        ThreadEvent::CredentialRequested {
            request_id,
            payload: payload(),
        },
        ThreadEvent::PluginInstallRequested {
            request_id,
            payload: payload(),
        },
        ThreadEvent::PluginUninstallRequested {
            request_id,
            payload: payload(),
        },
        ThreadEvent::EmailConfirmRequested {
            request_id,
            payload: payload(),
        },
        ThreadEvent::OAuthAuthorizationRequested {
            request_id,
            payload: payload(),
        },
        ThreadEvent::FormRequestResolved {
            request_id,
            outcome: FormRequestOutcome::Canceled,
        },
    ] {
        assert!(
            event.is_persisted(),
            "{} must be persisted",
            event.event_type()
        );
    }
}

#[test]
fn a_navigation_stays_transient() {
    assert!(!ThreadEvent::NavigationRequested {
        payload: "{}".to_string()
    }
    .is_persisted());
}

#[test]
fn context_captured_event_type_and_persistence() {
    let event = ThreadEvent::ContextCaptured {
        producer: crate::engine::thread_events::ContextProducer::MainLlm,
        model: "claude-opus-4-7".to_string(),
        context_window: 1_000_000,
        sections: vec![crate::engine::thread_events::ContextSection {
            name: "User Message".to_string(),
            content: Some("hi".to_string()),
            budget_delta_chars: 2,
            content_chars: Some(2),
            role: crate::engine::thread_events::ContextRole::User,
            group: None,
        }],
        tools: vec!["read_file".to_string()],
        estimated_total_tokens: 1,
        usage: Some(crate::llm::api_usage::ApiUsage {
            input_tokens: 12_345,
            output_tokens: 678,
            cache_read_tokens: 10_000,
            cache_creation_tokens: 200,
            modality: None,
            service_tier: None,
            cache_creation_1h_tokens: None,
            cache_creation_30m_tokens: None,
            cache_creation_12h_tokens: None,
            cache_creation_requested_ttl: None,
        }),
        trimmed: false,
        trim_passes: Vec::new(),
        purpose: crate::engine::thread_events::ContextPurpose::Turn,
        reconstructed: false,
        parent_tool_use_id: None,
        api_call_id: None,
        reasoning_effort: None,
        duration_ms: None,
        served_model: Some("claude-opus-4-7-20260101".to_string()),
        provider: None,
        stable_prefix_fingerprint: None,
        ttft_ms: None,
        request_id: None,
        telemetry_owned_by_cli: false,
        session: None,
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
    assert_eq!(serialized["served_model"], "claude-opus-4-7-20260101");

    let pre_call_json = r#"{
            "type":"ContextCaptured",
            "producer":"claude_code",
            "model":"claude-sonnet-4-6",
            "context_window":200000,
            "sections":[{"name":"Conversation","budget_delta_chars":500,"content_chars":1200}],
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
            assert_eq!(
                producer,
                crate::engine::thread_events::ContextProducer::ClaudeCode
            );
            assert!(usage.is_none(), "usage should be None pre-call");
            assert!(
                sections[0].content.is_none(),
                "content should be None when capture_context off"
            );
            assert_eq!(
                sections[0].content_chars,
                Some(1_200),
                "a dropped body does not move the region's measured size"
            );
        }
        other => panic!("expected ContextCaptured, got {other:?}"),
    }
}

#[test]
fn a_usage_without_the_1_hour_split_reads_as_none_and_writes_no_key() {
    let old: crate::llm::api_usage::ApiUsage = serde_json::from_str(
        r#"{"input_tokens":100,"output_tokens":5,"cache_read_tokens":0,"cache_creation_tokens":40}"#,
    )
    .unwrap();
    assert_eq!(old.cache_creation_1h_tokens, None);
    let written = serde_json::to_value(old).unwrap();
    assert!(written.get("cache_creation_1h_tokens").is_none());

    let split = crate::llm::api_usage::ApiUsage {
        cache_creation_1h_tokens: Some(40),
        ..old
    };
    assert_eq!(
        serde_json::to_value(split).unwrap()["cache_creation_1h_tokens"],
        40
    );
}

#[test]
fn a_capture_row_without_the_new_keys_reads_as_a_turn() {
    let legacy_json = r#"{
            "type":"ContextCaptured",
            "producer":"main_llm",
            "model":"claude-opus-4-7",
            "context_window":200000,
            "sections":[{"name":"Conversation","char_count":500}],
            "estimated_total_tokens":125,
            "usage":{"input_tokens":100,"output_tokens":5,
                     "cache_read_tokens":0,"cache_creation_tokens":0}
        }"#;
    let parsed: ThreadEvent = serde_json::from_str(legacy_json).unwrap();
    let ThreadEvent::ContextCaptured {
        purpose,
        reconstructed,
        sections,
        served_model,
        provider,
        ..
    } = parsed
    else {
        panic!("expected ContextCaptured");
    };
    assert_eq!(purpose, crate::engine::thread_events::ContextPurpose::Turn);
    assert!(!reconstructed, "a recorded row is not a reconstruction");
    assert_eq!(served_model, None, "an old row names no served model");
    assert_eq!(provider, None, "an old row names no provider");
    assert_eq!(sections[0].budget_delta_chars, 500);
    assert_eq!(sections[0].content_chars, None);
}

#[test]
fn a_capture_api_call_id_round_trips_and_is_omitted_when_absent() {
    let capture = |extra: &str| {
        format!(
            r#"{{"type":"ContextCaptured","producer":"claude_code","model":"claude-sonnet-5",
                "context_window":1000000,"sections":[],"estimated_total_tokens":438000{extra}}}"#
        )
    };
    let api_call_id_of = |json: &str| match serde_json::from_str::<ThreadEvent>(json).unwrap() {
        ThreadEvent::ContextCaptured { api_call_id, .. } => api_call_id,
        other => panic!("expected ContextCaptured, got {other:?}"),
    };

    let stamped = capture(r#","api_call_id":"msg_1""#);
    assert_eq!(api_call_id_of(&stamped).as_deref(), Some("msg_1"));
    let reserialized =
        serde_json::to_value(serde_json::from_str::<ThreadEvent>(&stamped).unwrap()).unwrap();
    assert_eq!(reserialized["api_call_id"], "msg_1");

    let legacy = capture("");
    assert_eq!(api_call_id_of(&legacy), None);
    let reserialized =
        serde_json::to_value(serde_json::from_str::<ThreadEvent>(&legacy).unwrap()).unwrap();
    assert!(reserialized.get("api_call_id").is_none());
}

#[test]
fn a_turn_capture_writes_neither_new_key() {
    let event = ThreadEvent::ContextCaptured {
        producer: crate::engine::thread_events::ContextProducer::MainLlm,
        model: "claude-opus-4-7".to_string(),
        context_window: 200_000,
        sections: vec![],
        tools: vec![],
        estimated_total_tokens: 1,
        usage: None,
        trimmed: false,
        trim_passes: Vec::new(),
        purpose: crate::engine::thread_events::ContextPurpose::Turn,
        reconstructed: false,
        parent_tool_use_id: None,
        api_call_id: None,
        reasoning_effort: None,
        duration_ms: None,
        served_model: None,
        provider: None,
        stable_prefix_fingerprint: None,
        ttft_ms: None,
        request_id: None,
        telemetry_owned_by_cli: false,
        session: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert!(json.get("purpose").is_none());
    assert!(json.get("reconstructed").is_none());
    assert!(json.get("served_model").is_none());
    assert!(json.get("provider").is_none());
    assert!(json.get("ttft_ms").is_none());
    assert!(json.get("request_id").is_none());
}

#[test]
fn a_capture_reads_with_or_without_the_transport_facts() {
    let with = |fields: &str| {
        format!(
            r#"{{"type":"ContextCaptured","producer":"main_llm","model":"m","context_window":0,
            "sections":[],"estimated_total_tokens":0{fields}}}"#
        )
    };
    let facts = |event: ThreadEvent| match event {
        ThreadEvent::ContextCaptured {
            ttft_ms,
            request_id,
            ..
        } => (ttft_ms, request_id),
        other => panic!("expected ContextCaptured, got {other:?}"),
    };
    let old: ThreadEvent = serde_json::from_str(&with("")).unwrap();
    assert_eq!(facts(old), (None, None));
    let new: ThreadEvent =
        serde_json::from_str(&with(r#","ttft_ms":412,"request_id":"req_abc""#)).unwrap();
    assert_eq!(facts(new), (Some(412), Some("req_abc".to_string())));
}

#[test]
fn a_model_call_failed_row_carries_facts_and_no_text() {
    let event = ThreadEvent::ModelCallFailed {
        model: "claude-opus-5-5".to_string(),
        purpose: crate::engine::thread_events::ContextPurpose::Turn,
        provider: Some("claude-gateway".to_string()),
        reasoning_effort: None,
        duration_ms: 900,
        attempts: 4,
        failure: crate::llm::call_trace::CallFailure::Http,
        status_code: Some(529),
        request_id: None,
    };
    assert_eq!(event.event_type(), "ModelCallFailed");
    assert!(event.is_persisted());
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ModelCallFailed");
    assert_eq!(json["failure"], "http");
    assert_eq!(json["status_code"], 529);
    assert!(json.get("purpose").is_none(), "a turn writes no purpose");
    assert!(json.get("request_id").is_none(), "absent stays absent");
    assert!(json.get("reasoning_effort").is_none());
    let strings: Vec<&str> = json
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|_| key.as_str()))
        .collect();
    assert_eq!(strings, ["failure", "model", "provider", "type"]);
    let ThreadEvent::ModelCallFailed {
        failure, attempts, ..
    } = serde_json::from_value(json).unwrap()
    else {
        panic!("expected ModelCallFailed");
    };
    assert_eq!(failure, crate::llm::call_trace::CallFailure::Http);
    assert_eq!(attempts, 4);
}

#[test]
fn an_auxiliary_capture_writes_its_purpose_and_producer() {
    let event = crate::engine::thread_events::aux_capture::auxiliary_capture(
        crate::engine::thread_events::ContextPurpose::Memory,
        "gemini-3-flash-preview",
        800,
        None,
        true,
    );
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["purpose"], "memory");
    assert_eq!(json["producer"], "auxiliary");
    assert_eq!(json["reconstructed"], serde_json::json!(true));
    assert!(
        json.get("usage").is_none(),
        "a reconstruction carries no measured usage"
    );
}

#[test]
fn every_purpose_survives_a_round_trip() {
    for (purpose, wire) in [
        (crate::engine::thread_events::ContextPurpose::Title, "title"),
        (
            crate::engine::thread_events::ContextPurpose::ImageDescribe,
            "image_describe",
        ),
        (
            crate::engine::thread_events::ContextPurpose::Memory,
            "memory",
        ),
        (
            crate::engine::thread_events::ContextPurpose::ConversationSummary,
            "conversation_summary",
        ),
        (
            crate::engine::thread_events::ContextPurpose::ImageGen,
            "image_gen",
        ),
    ] {
        let event = crate::engine::thread_events::aux_capture::auxiliary_capture(
            purpose, "m", 1, None, false,
        );
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["purpose"], wire);
        let parsed: ThreadEvent = serde_json::from_value(json).unwrap();
        let ThreadEvent::ContextCaptured {
            purpose: decoded, ..
        } = parsed
        else {
            panic!("expected ContextCaptured");
        };
        assert_eq!(decoded, purpose);
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
    let json = serde_json::to_value(&evt).unwrap();
    assert_eq!(json["type"], "ToolResult");
    assert_eq!(json["success"], serde_json::json!(true));
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

#[test]
fn child_thread_completed_event_round_trips() {
    let evt = ThreadEvent::ChildThreadCompleted {
        child_thread_id: uuid::Uuid::new_v4(),
        child_thread_title: Some("Nightly Step 1: Build & Test".into()),
        status: ChildCompletionStatus::Success,
        summary: "All green".into(),
        pending_change_ids: vec!["change-1".into()],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert_eq!(v["type"], "ChildThreadCompleted");
    let back: ThreadEvent = serde_json::from_value(v).unwrap();
    assert!(matches!(back, ThreadEvent::ChildThreadCompleted { .. }));
}

#[test]
fn child_thread_completed_event_type_is_pascal_case_name() {
    let evt = ThreadEvent::ChildThreadCompleted {
        child_thread_id: uuid::Uuid::new_v4(),
        child_thread_title: None,
        status: ChildCompletionStatus::NoChanges,
        summary: String::new(),
        pending_change_ids: vec![],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    };
    assert_eq!(evt.event_type(), "ChildThreadCompleted");
    assert!(evt.is_persisted());
}

#[test]
fn child_thread_completed_skips_empty_optional_fields() {
    let evt = ThreadEvent::ChildThreadCompleted {
        child_thread_id: uuid::Uuid::new_v4(),
        child_thread_title: None,
        status: ChildCompletionStatus::Failure,
        summary: "boom".into(),
        pending_change_ids: vec![],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert!(v.get("child_thread_title").is_none());
    assert!(v.get("pending_change_ids").is_none());
    assert_eq!(v["status"], "failure");
}

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

#[test]
fn child_thread_completed_indexable_text_returns_summary() {
    let evt = ThreadEvent::ChildThreadCompleted {
        child_thread_id: uuid::Uuid::new_v4(),
        child_thread_title: Some("Title".into()),
        status: ChildCompletionStatus::Success,
        summary: "deployment finished cleanly".into(),
        pending_change_ids: vec![],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    };
    assert_eq!(evt.indexable_text(), Some("deployment finished cleanly"));
}

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
fn context_kept_open_event_round_trips() {
    let id = uuid::Uuid::new_v4();
    let evt = ThreadEvent::ContextKeptOpen {
        kept_open_event_id: id,
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert_eq!(v["type"], "ContextKeptOpen");
    assert_eq!(v["kept_open_event_id"], id.to_string());
    let back: ThreadEvent = serde_json::from_value(v).unwrap();
    assert!(matches!(back, ThreadEvent::ContextKeptOpen { .. }));
    assert_eq!(evt.event_type(), "ContextKeptOpen");
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
    assert_eq!(evt.indexable_text(), Some("A cat sitting on a chair"));
}

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
        coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
        parent_tool_use_id: None,
        progress_note: false,
    }
    .is_per_token_streaming());
}

#[test]
fn is_per_token_streaming_allows_per_action_lifecycle_and_blocking_request_variants() {
    assert!(!ThreadEvent::UserQuestionAsked {
        tool_use_id: String::new(),
        cc_session_id: String::new(),
        question: String::new(),
        options: Vec::new(),
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
        permission_request: None,
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::CodingAgentPermissionRequest {
        request_id: String::new(),
        tool_use_id: String::new(),
        tool_name: String::new(),
        input: serde_json::Value::Null,
        summary: String::new(),
        grant_offer: None,
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::CredentialRequested {
        request_id: uuid::Uuid::nil(),
        payload: String::new(),
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::McpConsentRequested {
        tool: String::new(),
        args: serde_json::Value::Null,
    }
    .is_per_token_streaming());
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
        coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
        tool_use_id: String::new(),
        parent_tool_use_id: None,
        api_call_id: None,
    }
    .is_per_token_streaming());
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
        coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
        turn_end: None,
    }
    .is_per_token_streaming());
    assert!(!ThreadEvent::ThreadArchived.is_per_token_streaming());
}

fn voice_started(session_id: uuid::Uuid) -> ThreadEvent {
    ThreadEvent::VoiceSessionStarted { session_id }
}

fn voice_ended(session_id: uuid::Uuid, reason: VoiceSessionEndReason) -> ThreadEvent {
    ThreadEvent::VoiceSessionEnded {
        session_id,
        reason,
        duration_secs: 42,
    }
}

#[test]
fn voice_session_events_report_their_pascal_case_names() {
    let id = uuid::Uuid::new_v4();
    assert_eq!(voice_started(id).event_type(), "VoiceSessionStarted");
    assert_eq!(
        voice_ended(id, VoiceSessionEndReason::Hangup).event_type(),
        "VoiceSessionEnded"
    );
}

#[test]
fn voice_session_events_are_persisted_and_triggerable() {
    let id = uuid::Uuid::new_v4();
    for event in [
        voice_started(id),
        voice_ended(id, VoiceSessionEndReason::Hangup),
    ] {
        assert!(event.is_persisted(), "{} must persist", event.event_type());
        assert!(
            !event.is_per_token_streaming(),
            "{} must stay triggerable",
            event.event_type()
        );
    }
}

#[test]
fn a_voice_session_payload_carries_no_audio() {
    let id = uuid::Uuid::new_v4();
    let started = serde_json::to_value(voice_started(id)).unwrap();
    let mut started_keys: Vec<_> = started.as_object().unwrap().keys().cloned().collect();
    started_keys.sort();
    assert_eq!(started_keys, vec!["session_id", "type"]);

    let ended =
        serde_json::to_value(voice_ended(id, VoiceSessionEndReason::ProviderFailed)).unwrap();
    let mut ended_keys: Vec<_> = ended.as_object().unwrap().keys().cloned().collect();
    ended_keys.sort();
    assert_eq!(
        ended_keys,
        vec!["duration_secs", "reason", "session_id", "type"]
    );
    assert_eq!(ended["reason"], "provider_failed");
}

#[test]
fn a_voice_session_pairs_by_id_across_the_wire() {
    let id = uuid::Uuid::new_v4();
    let json =
        serde_json::to_value(voice_ended(id, VoiceSessionEndReason::EngineShutdown)).unwrap();
    let back: ThreadEvent = serde_json::from_value(json).unwrap();
    match back {
        ThreadEvent::VoiceSessionEnded {
            session_id,
            reason,
            duration_secs,
        } => {
            assert_eq!(session_id, id);
            assert_eq!(reason, VoiceSessionEndReason::EngineShutdown);
            assert_eq!(duration_secs, 42);
        }
        other => panic!("expected VoiceSessionEnded, got {}", other.event_type()),
    }
}

#[test]
fn a_voice_session_moves_neither_status_nor_section() {
    use crate::engine::thread_lifecycle::{classify_event, EventClass};
    assert_eq!(
        classify_event("VoiceSessionStarted"),
        Some(EventClass::Metadata)
    );
    assert_eq!(
        classify_event("VoiceSessionEnded"),
        Some(EventClass::Metadata)
    );
}

#[test]
fn widget_events_carry_the_app_id_and_nothing_else() {
    let events = [
        (
            ThreadEvent::WidgetShown {
                app_id: "fare-grid".into(),
                params: None,
                label: None,
            },
            "WidgetShown",
        ),
        (
            ThreadEvent::WidgetPinned {
                app_id: "fare-grid".into(),
                params: None,
                label: None,
                shown_event_id: None,
            },
            "WidgetPinned",
        ),
        (
            ThreadEvent::WidgetUnpinned {
                app_id: "fare-grid".into(),
                params: None,
                label: None,
            },
            "WidgetUnpinned",
        ),
    ];
    for (evt, name) in events {
        let v = serde_json::to_value(&evt).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 2, "{name} payload is type plus app_id: {v}");
        assert_eq!(v["type"], name);
        assert_eq!(v["app_id"], "fare-grid");
        let back: ThreadEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back.event_type(), name);
        assert!(evt.is_persisted());
    }
}

#[test]
fn an_inline_widget_size_round_trips() {
    use crate::engine::thread_lifecycle::{classify_event, EventClass};
    let evt = ThreadEvent::InlineWidgetSized {
        app_id: "player".into(),
        params: Some([("clip".to_string(), serde_json::json!("a.mp3"))].into()),
        size: crate::core::apps::WidgetSize::Large,
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "type": "InlineWidgetSized",
            "app_id": "player",
            "params": {"clip": "a.mp3"},
            "size": "large"
        })
    );
    assert!(matches!(
        serde_json::from_value::<ThreadEvent>(v).unwrap(),
        ThreadEvent::InlineWidgetSized { ref app_id, params: Some(_), size: crate::core::apps::WidgetSize::Large }
            if app_id == "player"
    ));
    assert!(evt.is_persisted());
    assert_eq!(
        classify_event("InlineWidgetSized"),
        Some(EventClass::Metadata)
    );
}
