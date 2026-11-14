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
                preview: Some("![first](artifacts/first.png)".into()),
                widget: None,
                quiet: false,
                quiet_reply: None,
            },
            QuestionOption {
                id: "o2".into(),
                label: "Second".into(),
                description: None,
                preview: None,
                widget: None,
                quiet: false,
                quiet_reply: None,
            },
        ],
        worktree_path: Some("/tmp/cc-abc".into()),
        multi_select: false,
        owner_approval: None,
        unattested_act: None,
        permission_request: None,
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
    assert_eq!(v["options"][0]["preview"], "![first](artifacts/first.png)");
    assert!(
        v["options"][1].get("preview").is_none(),
        "None preview should be skipped"
    );
    assert_eq!(v["worktree_path"], "/tmp/cc-abc");
}

/// Events are immutable, so every card stored before `preview` existed must
/// still decode.
#[test]
fn question_option_without_preview_still_decodes() {
    let option: QuestionOption =
        serde_json::from_value(serde_json::json!({ "id": "opt-0", "label": "A" })).unwrap();
    assert_eq!(option.preview, None);
}

/// Every answer stored before answers carried images must still decode, and
/// an imageless answer must serialize exactly as it did.
#[test]
fn answers_without_images_keep_their_old_shape() {
    let old_free: AnswerKind =
        serde_json::from_value(serde_json::json!({ "kind": "FreeText", "text": "yes" })).unwrap();
    assert_eq!(old_free.image_hashes(), &[] as &[String]);
    assert_eq!(
        serde_json::to_value(&old_free).unwrap(),
        serde_json::json!({ "kind": "FreeText", "text": "yes" })
    );
    let old_multi: AnswerKind = serde_json::from_value(
        serde_json::json!({ "kind": "MultiSelected", "option_ids": ["opt-0"] }),
    )
    .unwrap();
    assert_eq!(old_multi.image_hashes(), &[] as &[String]);
}

/// Every side question stored before asks carried images must still decode.
#[test]
fn a_side_question_asked_without_images_still_decodes() {
    let raw = r#"{"type":"SideQuestionAsked","side_question_id":"7f1c1b1e-5e2a-4f3a-9d77-3c0f2b9a8a11","question":"q"}"#;
    match serde_json::from_str::<ThreadEvent>(raw).expect("legacy parses") {
        ThreadEvent::SideQuestionAsked { image_hashes, .. } => assert!(image_hashes.is_empty()),
        other => panic!("expected SideQuestionAsked, got {other:?}"),
    }
}

#[test]
fn answer_images_round_trip() {
    let answer = AnswerKind::FreeText {
        text: String::new(),
        image_hashes: vec!["a".repeat(64)],
    };
    let v = serde_json::to_value(&answer).unwrap();
    assert_eq!(v["image_hashes"][0], "a".repeat(64));
    let back: AnswerKind = serde_json::from_value(v).unwrap();
    assert_eq!(back, answer);
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
        owner_approval: None,
        unattested_act: None,
        permission_request: None,
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
        owner_approval: None,
        unattested_act: None,
        permission_request: None,
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
        owner_approval: None,
        unattested_act: None,
        permission_request: None,
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
        compose: None,
        tool_use_id: "tu_1".into(),
        quiet: false,
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
        compose: None,
        tool_use_id: "tu_1".into(),
        quiet: false,
        answer: AnswerKind::FreeText {
            text: "let's do X".into(),
            image_hashes: vec![],
        },
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["answer"]["kind"], "FreeText");
    assert_eq!(v["answer"]["text"], "let's do X");
}

#[test]
fn user_question_answered_canceled_serialization() {
    let event = ThreadEvent::UserQuestionAnswered {
        compose: None,
        tool_use_id: "tu_1".into(),
        quiet: false,
        answer: AnswerKind::Canceled,
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["answer"]["kind"], "Canceled");
}

#[test]
fn user_question_answered_multi_selected_serialization() {
    let event = ThreadEvent::UserQuestionAnswered {
        compose: None,
        tool_use_id: "tu_1".into(),
        quiet: false,
        answer: AnswerKind::MultiSelected {
            option_ids: vec!["opt-0".into(), "opt-2".into()],
            text: None,
            image_hashes: vec![],
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
            answer: AnswerKind::MultiSelected {
                option_ids, text, ..
            },
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
        compose: None,
        tool_use_id: "tu_2".into(),
        quiet: false,
        answer: AnswerKind::MultiSelected {
            option_ids: vec!["opt-0".into()],
            text: Some("plus this".into()),
            image_hashes: vec![],
        },
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["answer"]["text"], "plus this");

    let raw = serde_json::to_string(&event).unwrap();
    let parsed: ThreadEvent = serde_json::from_str(&raw).expect("parse");
    match parsed {
        ThreadEvent::UserQuestionAnswered {
            answer: AnswerKind::MultiSelected {
                option_ids, text, ..
            },
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
        compose: None,
        tool_use_id: "tu_1".into(),
        quiet: false,
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

/// Stored rows of the retired single-use card still read, though nothing
/// emits them now (ADR 0387 history).
#[test]
fn retired_allow_once_rows_still_deserialize() {
    let thread = uuid::Uuid::new_v4();
    let cases = [
        serde_json::json!({
            "type": "OwnerApprovalRequested", "request_id": "owner-approval:abc",
            "approval": { "verb": "apply", "target_thread_id": thread },
            "question": "Let this thread act outside its own subtree, once?"
        }),
        serde_json::json!({
            "type": "OwnerApprovalSpent", "tool_use_id": "toolu-1#q0",
            "verb": "restore-snapshot", "target_workspace_id": "demo",
            "snapshot_handle": "snap-1"
        }),
        serde_json::json!({ "type": "UnattestedActSpent", "tool_use_id": "toolu-2#q0" }),
        serde_json::json!({
            "type": "UserQuestionAsked", "tool_use_id": "toolu-3#q0", "cc_session_id": "s",
            "question": "q", "multi_select": false,
            "owner_approval": { "verb": "archive", "target_thread_id": thread }
        }),
    ];
    for stored in cases {
        let event: ThreadEvent = serde_json::from_value(stored.clone())
            .unwrap_or_else(|e| panic!("{stored} no longer reads: {e}"));
        assert_eq!(serde_json::to_value(&event).unwrap(), stored);
    }
}

/// A stored thread card names no workspace or snapshot.
#[test]
fn legacy_owner_approval_decodes_without_snapshot_authority() {
    let old: OwnerApproval = serde_json::from_value(serde_json::json!({
        "verb": "apply", "target_thread_id": uuid::Uuid::new_v4()
    }))
    .unwrap();
    assert!(old.target_workspace_id.is_none());
    assert!(old.snapshot_handle.is_none());
}

/// The stored spellings of the retired verbs still read, so an old card's
/// timeline still renders.
#[test]
fn a_retired_verb_still_reads_from_a_stored_card() {
    use crate::engine::permission::Action;
    for (wire, verb) in [
        ("create-top-thread", Action::CreateTopThread),
        ("detach-child", Action::DetachChild),
    ] {
        let stored: OwnerApproval =
            serde_json::from_value(serde_json::json!({ "verb": wire })).unwrap();
        assert_eq!(stored.verb, verb);
        assert_eq!(stored.target_thread_id, None);
        assert_eq!(serde_json::to_value(verb).unwrap(), wire);
    }
}

/// No code emits a retired event or writes a retired card field. Their
/// variants stay only so stored rows read.
#[test]
fn nothing_emits_a_retired_allow_once_event() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let retired = [
        "ThreadEvent::OwnerApprovalRequested {",
        "ThreadEvent::OwnerApprovalSpent {",
        "ThreadEvent::UnattestedActSpent {",
        "owner_approval: Some(",
        "unattested_act: Some(",
    ];
    // A match pattern reads a variant, and a boxed act is the live
    // `permission_request.unattested_act`, not the retired top-level field.
    let reads = |line: &str| line.contains("{ .. }") || line.contains("Some(Box::new(");
    let mut hits = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let is_test = rel.contains("test") || rel.ends_with("/tests.rs");
            if path.extension().is_none_or(|e| e != "rs") || is_test {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for line in text.lines().filter(|l| !reads(l)) {
                if retired.iter().any(|needle| line.contains(needle)) {
                    hits.push(format!("{rel}: {}", line.trim()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "retired emits remain: {hits:#?}");
}
