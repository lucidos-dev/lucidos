use super::msg_helpers::*;
use super::*;

#[test]
fn completed_response_not_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "hello", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "working...", "request_id": "s1"}),
            1,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "working...", "channel": "claude_code", "request_id": "s1"}),
            2,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].completed, Some(true));
}

#[test]
fn followup_mid_stream_marks_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "do something", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "partial response", "request_id": "s1"}),
            1,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "actually do this instead", "request_id": "s1"}),
            2,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "new response", "request_id": "s1"}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "new response", "channel": "claude_code", "request_id": "s1"}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);

    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "do something");

    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "partial response");
    assert_eq!(
        msgs[1].completed,
        Some(false),
        "interrupted response must have completed=false"
    );

    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[2].content, "actually do this instead");

    assert_eq!(msgs[3].role, "assistant");
    assert_eq!(msgs[3].completed, Some(true));
}

#[test]
fn trailing_buffer_not_marked_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "hello", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "still working...", "request_id": "s1"}),
            1,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "still working...");
    assert_eq!(
        msgs[1].completed, None,
        "trailing buffer must have completed=None, not false"
    );
}

#[test]
fn interrupted_then_trailing_buffer() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "msg1", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "response1", "request_id": "s1"}),
            1,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "msg2", "request_id": "s1"}),
            2,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "response2", "request_id": "s1"}),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);

    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(
        msgs[1].completed,
        Some(false),
        "first response was interrupted"
    );

    assert_eq!(msgs[3].role, "assistant");
    assert_eq!(
        msgs[3].completed, None,
        "second response is trailing buffer, not interrupted"
    );
}

#[test]
fn followup_after_completed_not_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "msg1", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "resp1", "request_id": "s1"}),
            1,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp1", "channel": "claude_code", "request_id": "s1"}),
            2,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "msg2", "request_id": "s1"}),
            3,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "resp2", "request_id": "s1"}),
            4,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp2", "channel": "claude_code", "request_id": "s1"}),
            5,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
    assert_eq!(
        msgs[1].completed,
        Some(true),
        "first response completed normally"
    );
    assert_eq!(
        msgs[3].completed,
        Some(true),
        "second response completed normally"
    );
}

#[test]
fn text_streamed_followup_mid_stream_marks_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "q1", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": "partial", "request_id": "s1"}),
            1,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "q2", "request_id": "s1"}),
            2,
        ),
        make_event(
            "TextStreamed",
            json!({"text": "answer", "request_id": "s1"}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "answer", "request_id": "s1"}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
    assert_eq!(
        msgs[1].completed,
        Some(false),
        "text-streamed response was interrupted"
    );
    assert_eq!(msgs[3].completed, Some(true));
}

#[test]
fn response_failed_produces_error_message() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "hello"}), 0),
        make_event("ResponseFailed", json!({"error": "Model not found"}), 1),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[1].role, "assistant");
    assert!(
        msgs[1].content.starts_with("[ERROR]"),
        "ResponseFailed content should start with [ERROR] prefix, got: {}",
        msgs[1].content
    );
    assert!(msgs[1].content.contains("Model not found"));
    assert_eq!(msgs[1].completed, Some(true));
}

#[test]
fn response_failed_prefers_payload_request_event_id() {
    let user1_id = Uuid::new_v4();
    let user2_id = Uuid::new_v4();
    let events = vec![
        EventRow {
            id: user1_id,
            event_type: "MessageReceived".to_string(),
            payload: json!({"text": "first message"}),
            created: Utc.timestamp_opt(1700000000, 0).unwrap(),
            thread_id: None,
            sequence: None,
        },
        EventRow {
            id: user2_id,
            event_type: "MessageReceived".to_string(),
            payload: json!({"text": "second message"}),
            created: Utc.timestamp_opt(1700000001, 0).unwrap(),
            thread_id: None,
            sequence: None,
        },
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ResponseFailed".to_string(),
            payload: json!({"error": "API error", "request_event_id": user1_id.to_string()}),
            created: Utc.timestamp_opt(1700000002, 0).unwrap(),
            thread_id: None,
            sequence: None,
        },
    ];
    let msgs = build_session_messages(&events);
    let failed = msgs
        .iter()
        .find(|m| m.role == "assistant" && m.content.contains("API error"))
        .unwrap();
    assert_eq!(failed.request_event_id, Some(user1_id.to_string()),
            "ResponseFailed should use request_event_id from payload, not positional tracking (which would give user2)");
}
