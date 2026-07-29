use super::*;

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
