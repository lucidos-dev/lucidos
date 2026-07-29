use super::*;
use serde_json::json;

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
        channel: Some(EventChannel::ClaudeCode),
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

/// Image descriptions carry real content the user shared (a screenshot, a
/// ticket, a photo). Memory indexing keys off `indexable_text()`, so the
/// description must surface there — otherwise a "what's this?" + screenshot
/// turn stores nothing about the image. The title path already folds the
/// description in; memory must too.
#[test]
fn indexable_text_includes_image_description() {
    let described = ThreadEvent::ImageDescribed {
        source_event_id: uuid::Uuid::new_v4(),
        hash: "abc123".into(),
        description: "A movie ticket for Super Mario Galaxy at ODEON".into(),
        model: "gemini-3-flash-preview".into(),
    };
    assert_eq!(
        described.indexable_text(),
        Some("A movie ticket for Super Mario Galaxy at ODEON")
    );
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
        tool_called_event_id: None,
    }
    .indexable_text()
    .is_none());
    assert!(ThreadEvent::SessionStarted {
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        session_id: "s".into(),
        branch: String::new(),
        repo_id: None,
        coding_agent_kind: Default::default(),
        coding_agent_folder: String::new(),
        app_id: None,
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

