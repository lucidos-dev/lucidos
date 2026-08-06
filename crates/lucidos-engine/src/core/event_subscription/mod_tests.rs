use super::*;
use serde_json::json;

fn sub(event_type: &str, condition: Option<Value>) -> EventSubscription {
    EventSubscription {
        event_type: event_type.to_string(),
        condition,
    }
}

// ── matches / any_matches ───────────────────────────────────────────

#[test]
fn matches_requires_both_name_and_condition() {
    let s = sub("ChangeProposed", Some(json!({"file_count": {"$gt": 0}})));
    assert!(s.matches("ChangeProposed", &json!({"file_count": 3})));
    // Right name, condition false.
    assert!(!s.matches("ChangeProposed", &json!({"file_count": 0})));
    // Condition true, wrong name.
    assert!(!s.matches("ChangeApplied", &json!({"file_count": 3})));
}

#[test]
fn matches_with_no_condition_is_name_only() {
    let s = sub("ChangeProposed", None);
    assert!(s.matches("ChangeProposed", &json!({"anything": true})));
    assert!(!s.matches("ChangeApplied", &json!({"anything": true})));
}

#[test]
fn any_matches_is_per_entry_or() {
    let subs = vec![
        sub("ChangeProposed", Some(json!({"file_count": {"$gt": 5}}))),
        sub("ResponseGenerated", None),
    ];
    // Second entry matches even though the first entry's condition fails, and
    // the first entry's condition must NOT constrain the second entry's event.
    assert!(EventSubscription::any_matches(
        &subs,
        "ResponseGenerated",
        &json!({"text": "done"})
    ));
    assert!(!EventSubscription::any_matches(
        &subs,
        "ChangeProposed",
        &json!({"file_count": 2})
    ));
    assert!(EventSubscription::any_matches(
        &subs,
        "ChangeProposed",
        &json!({"file_count": 9})
    ));
}

#[test]
fn any_matches_on_empty_list_is_false() {
    assert!(!EventSubscription::any_matches(
        &[],
        "ChangeProposed",
        &json!({})
    ));
}

/// I8: the trigger matcher and the event-wait dispatcher must return the same
/// verdict for every (subscription, event) pair. Both go through
/// `EventSubscription::matches`, so this table pins the shared behavior; the
/// trigger side re-runs the identical table through
/// `find_matching_event_triggers` in `triggers::tests` to prove it did not
/// re-implement the predicate.
pub(crate) const PARITY_CASES: &[(&str, Option<&str>, &str, &str, bool)] = &[
    // (sub event_type, sub condition JSON, event type, event payload JSON, expected)
    ("ChangeProposed", None, "ChangeProposed", r#"{}"#, true),
    ("ChangeProposed", None, "ChangeApplied", r#"{}"#, false),
    (
        "ChangeProposed",
        Some(r#"{"file_count": {"$gt": 2}}"#),
        "ChangeProposed",
        r#"{"file_count": 3}"#,
        true,
    ),
    (
        "ChangeProposed",
        Some(r#"{"file_count": {"$gt": 2}}"#),
        "ChangeProposed",
        r#"{"file_count": 1}"#,
        false,
    ),
    (
        "ResponseGenerated",
        Some(r#"{"model": {"$in": ["a", "b"]}}"#),
        "ResponseGenerated",
        r#"{"model": "b"}"#,
        true,
    ),
    (
        "ResponseGenerated",
        Some(r#"{"model": {"$in": ["a", "b"]}}"#),
        "ResponseGenerated",
        r#"{"model": "c"}"#,
        false,
    ),
    // A missing field never matches, rather than matching vacuously.
    (
        "ToolCalled",
        Some(r#"{"name": "run_bash"}"#),
        "ToolCalled",
        r#"{"args": {}}"#,
        false,
    ),
    (
        "ToolCalled",
        Some(r#"{"name": "run_bash"}"#),
        "ToolCalled",
        r#"{"name": "run_bash"}"#,
        true,
    ),
];

#[test]
fn parity_table_holds_for_the_shared_matcher() {
    for (sub_type, cond, event_type, payload, expected) in PARITY_CASES {
        let s = sub(sub_type, cond.map(|c| serde_json::from_str(c).unwrap()));
        let payload: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            s.matches(event_type, &payload),
            *expected,
            "subscription {sub_type} cond={cond:?} vs event {event_type} {payload}",
        );
    }
}

// ── normalize_list / from_object_entry ──────────────────────────────

#[test]
fn normalize_list_trims_and_drops_blank_names() {
    let out = EventSubscription::normalize_list(vec![
        sub("  ChangeProposed  ", None),
        sub("   ", Some(json!({"x": 1}))),
        sub("", None),
        sub("ResponseGenerated", Some(json!({"x": 1}))),
    ]);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].event_type, "ChangeProposed");
    assert_eq!(out[1].event_type, "ResponseGenerated");
    assert_eq!(out[1].condition, Some(json!({"x": 1})));
}

#[test]
fn from_object_entry_reads_name_and_optional_condition() {
    let obj = json!({"event_type": " ChangeProposed ", "condition": {"a": 1}});
    let s = EventSubscription::from_object_entry(obj.as_object().unwrap()).unwrap();
    assert_eq!(s.event_type, "ChangeProposed");
    assert_eq!(s.condition, Some(json!({"a": 1})));

    // An explicit null condition is the same as absent.
    let obj = json!({"event_type": "X", "condition": null});
    let s = EventSubscription::from_object_entry(obj.as_object().unwrap()).unwrap();
    assert_eq!(s.condition, None);

    // Missing or blank name is rejected.
    assert!(EventSubscription::from_object_entry(json!({}).as_object().unwrap()).is_none());
    assert!(
        EventSubscription::from_object_entry(json!({"event_type": "  "}).as_object().unwrap())
            .is_none()
    );
}

// ── the subscribability gate (I9) ───────────────────────────────────

/// The name constant and the predicate are two spellings of one blocklist, so
/// a variant added to one and not the other is a silent hole: the dispatcher
/// would drop the event while the tool still accepted a wait on it.
#[test]
fn per_token_streaming_names_match_the_predicate() {
    let streaming: Vec<ThreadEvent> = vec![
        ThreadEvent::TextStreamed {
            text: String::new(),
        },
        ThreadEvent::ThoughtStreamed {
            text: String::new(),
        },
        ThreadEvent::CodingAgentTextStreamed {
            text: String::new(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        ThreadEvent::CodingAgentThoughtStreamed {
            text: String::new(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
    ];
    assert_eq!(
        streaming.len(),
        PER_TOKEN_STREAMING_EVENT_TYPES.len(),
        "PER_TOKEN_STREAMING_EVENT_TYPES and is_per_token_streaming disagree on how \
         many variants are blocked",
    );
    for event in &streaming {
        assert!(
            event.is_per_token_streaming(),
            "{} is listed in PER_TOKEN_STREAMING_EVENT_TYPES but the predicate says otherwise",
            event.event_type(),
        );
        assert!(
            PER_TOKEN_STREAMING_EVENT_TYPES.contains(&event.event_type()),
            "{} is per-token streaming but missing from PER_TOKEN_STREAMING_EVENT_TYPES",
            event.event_type(),
        );
        assert!(!is_subscribable(event), "the gate must drop the firehose");
    }
}

#[test]
fn the_gate_admits_ordinary_lifecycle_and_per_action_events() {
    // Per-action variants are high cardinality but deliberately subscribable:
    // a subscriber scopes them with a `condition:` rather than being refused.
    assert!(is_subscribable(&ThreadEvent::ToolCalled {
        name: "run_bash".to_string(),
        args: json!({}),
        description: String::new(),
    }));
    assert!(is_subscribable(&ThreadEvent::ThreadArchived));
}

#[test]
fn streaming_variants_are_refused_as_awaitable() {
    for name in PER_TOKEN_STREAMING_EVENT_TYPES {
        let err = validate_awaitable_event_type(name).unwrap_err();
        assert!(
            err.contains(name) && err.contains("per-token streaming"),
            "refusal for {name} should name the variant and why: {err}",
        );
    }
}

/// The `EventWait*` family is refused as awaitable (it would self-satisfy) but
/// stays *triggerable*: the gate is about which events reach the matchers, and
/// it does not drop them.
#[test]
fn event_wait_variants_are_refused_as_awaitable_but_not_gated() {
    for name in EVENT_WAIT_EVENT_TYPES {
        let err = validate_awaitable_event_type(name).unwrap_err();
        assert!(
            err.contains(name),
            "refusal for {name} should name the variant: {err}",
        );
        assert!(
            !PER_TOKEN_STREAMING_EVENT_TYPES.contains(name),
            "{name} must not also be on the streaming blocklist",
        );
    }
}

#[test]
fn a_known_thread_event_name_is_awaitable() {
    assert_eq!(
        validate_awaitable_event_type("ChangeProposed"),
        Ok(AwaitableVerdict::KnownThreadEvent),
    );
    // Leading/trailing whitespace is tolerated, matching normalize_list.
    assert_eq!(
        validate_awaitable_event_type("  ResponseGenerated "),
        Ok(AwaitableVerdict::KnownThreadEvent),
    );
}

#[test]
fn an_unknown_name_is_accepted_for_the_caller_to_corroborate() {
    // A workspace-defined domain event nobody has emitted yet. Refusing these
    // would break the most valuable case for the tool.
    assert_eq!(
        validate_awaitable_event_type("ReleasePublished"),
        Ok(AwaitableVerdict::UnknownName),
    );
}

#[test]
fn an_empty_name_is_refused() {
    assert!(validate_awaitable_event_type("   ").is_err());
}
