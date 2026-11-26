use super::msg_helpers::*;
use super::*;

#[test]
fn messages_with_thread_id_preserved() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "msg1", "request_id": "r1", "thread_id": "t1"}),
            0,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp1", "request_id": "r1", "thread_id": "t1"}),
            1,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "msg2", "request_id": "r2", "thread_id": "t1"}),
            2,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp2", "request_id": "r2", "thread_id": "t1"}),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
}

#[test]
fn different_threads_build_independently() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "thread1-msg", "request_id": "r1", "thread_id": "t1"}),
            0,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "thread2-msg", "request_id": "r2", "thread_id": "t2"}),
            1,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp1", "request_id": "r1", "thread_id": "t1"}),
            2,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp2", "request_id": "r2", "thread_id": "t2"}),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].content, "thread1-msg");
    assert_eq!(msgs[1].content, "thread2-msg");
}

#[test]
fn follow_up_in_same_thread_different_request_ids() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "first question", "request_id": "r1", "thread_id": "t1"}),
            0,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "first answer", "request_id": "r1", "thread_id": "t1"}),
            1,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "follow-up question", "request_id": "r2", "thread_id": "t1"}),
            2,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "follow-up answer", "request_id": "r2", "thread_id": "t1"}),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "first question");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "first answer");
    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[2].content, "follow-up question");
    assert_eq!(msgs[3].role, "assistant");
    assert_eq!(msgs[3].content, "follow-up answer");
}

#[test]
fn trigger_text_does_not_contaminate_cc_response() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({
                "text": "explain changes",
                "channel": "claude_code",
                "thread_id": "cc-thread",
                "request_id": "r1"
            }),
            0,
        ),
        make_event("Thinking", json!({"thread_id": "cc-thread"}), 1),
        make_event(
            "TextStreamed",
            json!({
                "text": "Kontrollsløyfen er kjørt. Alle tre pumpene ble justert.",
                "thread_id": "sched-thread"
            }),
            2,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({
                "text": "Here are the changes explained.",
                "channel": "claude_code",
                "thread_id": "cc-thread"
            }),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({
                "text": "Here are the changes explained.",
                "channel": "claude_code",
                "thread_id": "cc-thread",
                "request_event_id": "cc-event-id"
            }),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);

    let cc_response = msgs
        .iter()
        .find(|m| m.role == "assistant" && m.channel.as_deref() == Some("claude_code"));
    assert!(cc_response.is_some(), "CC response should exist");
    let cc = cc_response.unwrap();
    assert_eq!(cc.content, "Here are the changes explained.");
    assert!(
        !cc.content.contains("Kontrollsløyfen"),
        "CC response must NOT contain scheduled trigger text"
    );
}

#[test]
fn result_text_extra_content_appears_in_events() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "run tests"}), 0),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "Let me run the tests.", "channel": "claude_code"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "Bash", "input": {"command": "cargo test"}}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "Bash", "result": "ok", "success": true}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({
                "text": "Let me run the tests.\n\nAll tests passed!",
                "channel": "claude_code"
            }),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    let assistant = &msgs[1];
    assert_eq!(assistant.role, "assistant");

    let text_events: Vec<_> = assistant
        .events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::Text { md } if !md.trim().is_empty() => Some(md.as_str()),
            _ => None,
        })
        .collect();
    let combined = text_events.join("");
    assert!(
        combined.contains("All tests passed!"),
        "Extra result text must appear in events. Got text events: {:?}",
        text_events,
    );
}

#[test]
fn cc_tool_calls_without_text_preserves_step_events() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "fix the bug", "channel": "claude_code"}),
            0,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "Read", "args": {"file_path": "src/main.rs"}}),
            1,
        ),
        make_event(
            "ToolResult",
            json!({"name": "Read", "result": "fn main() {}", "success": true}),
            2,
        ),
    ];
    let msgs = build_session_messages(&events);

    assert_eq!(
        msgs.len(),
        2,
        "Expected user + assistant message, got {}",
        msgs.len()
    );
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].completed, None);

    let step_events: Vec<_> = msgs[1]
        .events
        .iter()
        .filter(|e| matches!(e, ResponseEvent::Step { .. }))
        .collect();
    assert!(
        !step_events.is_empty(),
        "Step events must be preserved when CC has tool calls but no text. Got events: {:?}",
        msgs[1].events,
    );
}

#[test]
fn cc_tool_calls_with_text_still_works() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "fix the bug", "channel": "claude_code"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "Let me read the file."}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "Read", "args": {"file_path": "src/main.rs"}}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "Read", "result": "fn main() {}", "success": true}),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);

    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].completed, None);

    let has_text = msgs[1]
        .events
        .iter()
        .any(|e| matches!(e, ResponseEvent::Text { md } if !md.trim().is_empty()));
    let has_step = msgs[1]
        .events
        .iter()
        .any(|e| matches!(e, ResponseEvent::Step { .. }));
    assert!(has_text, "Text events expected");
    assert!(has_step, "Step events expected");
}

#[test]
fn build_session_messages_marks_tool_step_success_from_payload() {
    let events = vec![
        make_event(
            "ToolCalled",
            json!({"name": "load_knowhow", "args": {"id": "x"}, "description": "Loading"}),
            0,
        ),
        make_event(
            "ToolResult",
            json!({"name": "load_knowhow", "result": "body", "success": true}),
            1,
        ),
        make_event("ResponseGenerated", json!({"text": "ok"}), 2),
    ];
    let msgs = build_session_messages(&events);
    let assistant = msgs
        .iter()
        .find(|m| m.role == "assistant")
        .expect("assistant");
    assert_eq!(assistant.steps.len(), 1);
    assert!(assistant.steps[0].success, "step.success should be true");
}

#[test]
fn build_session_messages_legacy_tool_result_without_success_defaults_to_true() {
    let events = vec![
        make_event(
            "ToolCalled",
            json!({"name": "x", "args": {}, "description": "d"}),
            0,
        ),
        make_event("ToolResult", json!({"name": "x", "result": "ok"}), 1),
        make_event("ResponseGenerated", json!({"text": "ok"}), 2),
    ];
    let msgs = build_session_messages(&events);
    let assistant = msgs
        .iter()
        .find(|m| m.role == "assistant")
        .expect("assistant");
    assert!(
        assistant.steps[0].success,
        "missing success defaults to true"
    );
}

#[test]
fn a_guest_authored_response_carries_its_agent_into_the_projection() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "what's on today?"}), 0),
        make_event(
            "ResponseGenerated",
            json!({
                "text": "Let me look.",
                "actor": {"kind": "agent", "agent": {"kind": "guest", "label": "Voice"}}
            }),
            1,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = msgs
        .iter()
        .find(|m| m.role == "assistant")
        .expect("assistant");
    assert_eq!(
        assistant.agent,
        Some(crate::engine::thread_events::AgentParticipant::Guest {
            label: "Voice".into()
        })
    );
    let user = msgs.iter().find(|m| m.role == "user").expect("user");
    assert_eq!(user.agent, None);
}

#[test]
fn our_agent_is_named_and_a_legacy_row_names_nobody() {
    let events = vec![
        make_event(
            "ResponseGenerated",
            json!({"text": "ours", "actor": {"kind": "agent"}}),
            0,
        ),
        make_event("MessageReceived", json!({"text": "again"}), 1),
        make_event("ResponseGenerated", json!({"text": "legacy"}), 2),
    ];
    let msgs = build_session_messages(&events);
    let assistants: Vec<_> = msgs.iter().filter(|m| m.role == "assistant").collect();
    assert_eq!(assistants.len(), 2, "{msgs:?}");
    assert_eq!(
        assistants[0].agent,
        Some(crate::engine::thread_events::AgentParticipant::LucidosAgent)
    );
    assert_eq!(assistants[1].agent, None);
}

#[test]
fn a_human_actor_never_becomes_an_authoring_agent() {
    let events = vec![make_event(
        "ResponseGenerated",
        json!({
            "text": "hi",
            "actor": {"kind": "device", "device_id": "dev-1", "label": "My MacBook"}
        }),
        0,
    )];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs[0].agent, None);
}

#[test]
fn a_spoken_reply_reaches_the_doer_under_its_own_label() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "what have I got on?"}), 0),
        make_event(
            "SpokenReplyGenerated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": "Let me check.",
                "interrupted": false,
                "actor": {"kind": "agent", "agent": {"kind": "guest", "label": "Lucidos (aloud)"}}
            }),
            1,
        ),
    ];
    let msgs = build_session_messages(&events);
    let spoken = msgs
        .iter()
        .find(|m| m.content == "Let me check.")
        .expect("the spoken reply");
    assert_eq!(spoken.role, "assistant");
    assert_eq!(
        spoken.agent,
        Some(crate::engine::thread_events::AgentParticipant::Guest {
            label: "Lucidos (aloud)".into()
        })
    );
    assert_eq!(
        spoken.agent.as_ref().expect("an author").speaker_label(),
        "Lucidos (aloud)"
    );
}

#[test]
fn a_spoken_reply_takes_nothing_from_the_turn_around_it() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "what have I got on?"}), 0),
        make_event(
            "ToolCalled",
            json!({"name": "list_files", "args": {}, "tool_use_id": "t1"}),
            1,
        ),
        make_event(
            "SpokenReplyGenerated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": "Still looking.",
                "interrupted": false
            }),
            2,
        ),
        make_event("ResponseGenerated", json!({"text": "Two things."}), 3),
    ];
    let msgs = build_session_messages(&events);
    let spoken = msgs
        .iter()
        .find(|m| m.content == "Still looking.")
        .expect("the spoken reply");
    assert!(spoken.steps.is_empty(), "{:?}", spoken.steps);

    let answer = msgs
        .iter()
        .find(|m| m.content == "Two things.")
        .expect("the doer's answer");
    assert_eq!(answer.steps.len(), 1, "the tool call went missing");
    assert_eq!(answer.steps[0].tool_name.as_deref(), Some("list_files"));
}

#[test]
fn a_spoken_reply_with_no_words_makes_no_message() {
    let events = vec![make_event(
        "SpokenReplyGenerated",
        json!({
            "session_id": "11111111-1111-4111-8111-111111111111",
            "text": "   ",
            "interrupted": true
        }),
        0,
    )];
    assert!(build_session_messages(&events).is_empty());
}

#[test]
fn an_utterance_the_talker_handled_reaches_the_doer_as_the_user() {
    let events = vec![make_event(
        "SpokenMessageReceived",
        json!({
            "session_id": "11111111-1111-4111-8111-111111111111",
            "text": "hei"
        }),
        0,
    )];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "hei");
    assert_eq!(msgs[0].agent, None);
}

#[test]
fn the_pieces_of_one_breath_read_back_as_one_message() {
    let events: Vec<_> = ["So what I", "wanted to ask", "you is", "this."]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            make_event(
                "SpokenMessageReceived",
                json!({
                    "session_id": "11111111-1111-4111-8111-111111111111",
                    "text": text
                }),
                (i as i64) * 2,
            )
        })
        .collect();
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 1, "{:?}", msgs);
    assert_eq!(msgs[0].content, "So what I wanted to ask you is this.");
}

#[test]
fn a_delegation_mid_sentence_leaves_one_message() {
    let events = vec![
        make_event(
            "SpokenMessageReceived",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": "Just spawn the coding agent"
            }),
            0,
        ),
        make_event(
            "WorkDelegated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "reason": "they want a coding agent spawned"
            }),
            0,
        ),
        make_event(
            "SpokenMessageReceived",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": ", please"
            }),
            1,
        ),
    ];
    let msgs = build_session_messages(&events);
    let said: Vec<_> = msgs
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        said,
        vec!["Just spawn the coding agent, please"],
        "{:?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.content.contains("[Asked for you]")),
        "the ask went missing: {:?}",
        msgs
    );
}

#[test]
fn a_delegation_between_two_replies_still_cuts_them() {
    let events = vec![
        make_event(
            "SpokenReplyGenerated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": "One moment.",
                "interrupted": false
            }),
            0,
        ),
        make_event(
            "WorkDelegated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "reason": "they want the release checked"
            }),
            0,
        ),
        make_event(
            "SpokenReplyGenerated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": "Still on it.",
                "interrupted": false
            }),
            2,
        ),
    ];
    let msgs = build_session_messages(&events);
    let spoken: Vec<_> = msgs
        .iter()
        .filter(|m| m.content != "[Asked for you] they want the release checked")
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(spoken, vec!["One moment.", "Still on it."], "{:?}", msgs);
}

#[test]
fn a_turn_after_the_note_still_cuts_the_fragments() {
    let events = vec![
        make_event(
            "SpokenMessageReceived",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": "Status"
            }),
            0,
        ),
        make_event(
            "WorkDelegated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "reason": "they want the status"
            }),
            0,
        ),
        make_event("MessageReceived", json!({"text": "watch the build"}), 0),
        make_event(
            "SpokenMessageReceived",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": ", please"
            }),
            1,
        ),
    ];
    let msgs = build_session_messages(&events);
    let said: Vec<_> = msgs
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        said,
        vec!["Status", "watch the build", ", please"],
        "{:?}",
        msgs
    );
}

#[test]
fn a_turn_between_two_fragments_still_cuts_them() {
    let events = vec![
        make_event(
            "SpokenMessageReceived",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": "Status"
            }),
            0,
        ),
        make_event("MessageReceived", json!({"text": "watch the build"}), 0),
        make_event(
            "SpokenMessageReceived",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": ", please"
            }),
            1,
        ),
    ];
    let msgs = build_session_messages(&events);
    let said: Vec<_> = msgs
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        said,
        vec!["Status", "watch the build", ", please"],
        "{:?}",
        msgs
    );
}

#[test]
fn two_calls_never_read_back_as_one_sentence() {
    let events = vec![
        make_event(
            "SpokenMessageReceived",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "text": "and then check the"
            }),
            0,
        ),
        make_event(
            "SpokenMessageReceived",
            json!({
                "session_id": "22222222-2222-4222-8222-222222222222",
                "text": "release notes"
            }),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2, "{:?}", msgs);
    assert_eq!(msgs[0].content, "and then check the");
    assert_eq!(msgs[1].content, "release notes");
}

#[test]
fn a_delegation_tells_the_doer_what_it_was_asked_for() {
    let events = vec![
        make_event(
            "WorkDelegated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "reason": "they want today's threads",
                "actor": {"kind": "agent", "agent": {"kind": "guest", "label": "Lucidos (aloud)"}}
            }),
            0,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "what have I got running"}),
            1,
        ),
    ];
    let msgs = build_session_messages(&events);
    let asked = msgs
        .iter()
        .find(|m| m.content.contains("they want today's threads"))
        .expect("the delegation");
    assert_eq!(asked.role, "assistant");
    assert_eq!(
        asked.agent.as_ref().expect("an author").speaker_label(),
        "Lucidos (aloud)"
    );
}

#[test]
fn a_delegation_takes_nothing_from_the_turn_it_started() {
    let events = vec![
        make_event(
            "WorkDelegated",
            json!({
                "session_id": "11111111-1111-4111-8111-111111111111",
                "reason": "they want today's threads"
            }),
            0,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "what have I got running"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "list_files", "args": {}, "tool_use_id": "t1"}),
            2,
        ),
        make_event("ResponseGenerated", json!({"text": "Two things."}), 3),
    ];
    let msgs = build_session_messages(&events);
    let answer = msgs
        .iter()
        .find(|m| m.content == "Two things.")
        .expect("the doer's answer");
    assert_eq!(answer.steps.len(), 1, "the tool call went missing");
    assert_eq!(answer.steps[0].tool_name.as_deref(), Some("list_files"));
}

#[test]
fn side_questions_never_enter_session_history() {
    let turn = |with_side_questions: bool| {
        let mut events = vec![make_event(
            "MessageReceived",
            json!({"text": "fix the bug", "request_id": "r1"}),
            0,
        )];
        if with_side_questions {
            events.push(make_event(
                "SideQuestionAsked",
                json!({"side_question_id": Uuid::new_v4(), "question": "what is x?"}),
                1,
            ));
            events.push(make_event(
                "SideQuestionAnswered",
                json!({"side_question_id": Uuid::new_v4(), "answer": "x is 3"}),
                2,
            ));
            events.push(make_event(
                "SideQuestionFailed",
                json!({"side_question_id": Uuid::new_v4(), "error": "no answer"}),
                3,
            ));
            events.push(make_event(
                "SideQuestionDismissed",
                json!({"side_question_id": Uuid::new_v4()}),
                4,
            ));
        }
        events.push(make_event(
            "ResponseGenerated",
            json!({"text": "fixed", "request_id": "r1"}),
            5,
        ));
        let mut messages = serde_json::to_value(build_session_messages(&events)).unwrap();
        for message in messages.as_array_mut().unwrap() {
            let fields = message.as_object_mut().unwrap();
            fields.remove("event_id");
            fields.remove("request_event_id");
        }
        messages
    };
    let with = turn(true);
    assert_eq!(with, turn(false));
    let text = with.to_string();
    assert!(!text.contains("what is x?") && !text.contains("x is 3"));
}

#[test]
fn a_user_only_message_is_a_user_turn_that_opens_no_request() {
    let device = |id: &str| json!({"kind": "device", "device_id": id});
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "draft the plan", "origin": device("dev-sam")}),
            0,
        ),
        make_event("ResponseGenerated", json!({"text": "Here it is."}), 1),
        make_event(
            "UserOnlyMessagePosted",
            json!({"text": "looks thin to me", "actor": device("dev-alex")}),
            2,
        ),
        make_event(
            "UserOnlyMessagePosted",
            json!({"text": "agreed, add risks", "actor": device("dev-sam")}),
            3,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "add the risks", "origin": device("dev-sam")}),
            4,
        ),
        make_event("ResponseGenerated", json!({"text": "Added."}), 5),
    ];
    let msgs = build_session_messages(&events);
    let turns: Vec<(&str, &str, Option<&str>)> = msgs
        .iter()
        .map(|m| {
            (
                m.role.as_str(),
                m.content.as_str(),
                m.sender_device_id.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        turns,
        vec![
            ("user", "draft the plan", Some("dev-sam")),
            ("assistant", "Here it is.", None),
            ("user", "looks thin to me", Some("dev-alex")),
            ("user", "agreed, add risks", Some("dev-sam")),
            ("user", "add the risks", Some("dev-sam")),
            ("assistant", "Added.", None),
        ]
    );
    assert_eq!(
        msgs[5].request_event_id, msgs[4].event_id,
        "the reply answers the woken message, not a user-only one"
    );
}

#[test]
fn only_a_device_origin_names_a_sender() {
    let events = vec![make_event(
        "MessageReceived",
        json!({"text": "run the report", "origin": {"kind": "engine", "reason": {"kind": "orphan_recovery"}}}),
        0,
    )];
    assert_eq!(build_session_messages(&events)[0].sender_device_id, None);
}

#[test]
fn a_user_only_message_during_a_turn_follows_its_reply() {
    let device = |id: &str| json!({"kind": "device", "device_id": id});
    let asked = make_event(
        "MessageReceived",
        json!({"text": "draft the plan", "origin": device("dev-sam")}),
        0,
    );
    let streamed = make_event("TextStreamed", json!({"text": "Working on"}), 1);
    let aside = make_event(
        "UserOnlyMessagePosted",
        json!({"text": "keep it short", "actor": device("dev-alex")}),
        2,
    );
    let done = make_event("ResponseGenerated", json!({"text": "Here it is."}), 3);
    let roles = |events: &[EventRow]| -> Vec<(String, String)> {
        build_session_messages(events)
            .into_iter()
            .map(|m| (m.role, m.content))
            .collect()
    };
    let pair = |role: &str, content: &str| (role.to_string(), content.to_string());

    assert_eq!(
        roles(&[asked.clone(), streamed.clone(), aside.clone(), done]),
        vec![
            pair("user", "draft the plan"),
            pair("assistant", "Working on\n\nHere it is."),
            pair("user", "keep it short"),
        ]
    );
    assert_eq!(
        roles(&[asked, streamed, aside]),
        vec![
            pair("user", "draft the plan"),
            pair("user", "keep it short"),
            pair("assistant", "Working on"),
        ]
    );
}
