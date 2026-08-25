use super::*;
use serde_json::json;

fn sub(event_type: &str, condition: Option<Value>) -> EventSubscription {
    EventSubscription {
        event_type: event_type.to_string(),
        condition,
    }
}

/// Validate for the **wait** surface, the stricter of the two. Only the
/// `EventWait*` family differs, so these tests read the wait surface as the
/// default and name the trigger one where the two part company.
fn awaitable(name: &str) -> Result<SubscriptionVerdict, String> {
    validate_subscribable_event_type(name, SubscriptionSurface::Wait)
}

/// Validate for the **trigger** surface, a strict superset of the wait one.
fn triggerable(name: &str) -> Result<SubscriptionVerdict, String> {
    validate_subscribable_event_type(name, SubscriptionSurface::Trigger)
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
    // A persisted system event (ADR 0113). Both matchers see it, and both see
    // the flattened view, so `error` is a field a condition can name.
    (
        "BackupFailed",
        None,
        "BackupFailed",
        r#"{"error": "x"}"#,
        true,
    ),
    (
        "BackupCompleted",
        None,
        "BackupFailed",
        r#"{"error": "x"}"#,
        false,
    ),
    (
        "BackupFailed",
        Some(r#"{"error": "disk full"}"#),
        "BackupFailed",
        r#"{"error": "disk full"}"#,
        true,
    ),
    // The same event, un-flattened. Both matchers must be equally blind to a
    // field buried under `data`, which is why the view flattens upstream.
    (
        "BackupFailed",
        Some(r#"{"error": "disk full"}"#),
        "BackupFailed",
        r#"{"type": "BackupFailed", "data": {"error": "disk full"}}"#,
        false,
    ),
    // A field path. The live case: `action` is top level, the rest is nested.
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"action": "completed", "workflow_run.event": "schedule"}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"action": "completed", "workflow_run": {"event": "schedule"}}"#,
        true,
    ),
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"action": "completed", "workflow_run.event": "schedule"}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"action": "completed", "workflow_run": {"event": "workflow_dispatch"}}"#,
        false,
    ),
    // A path that resolves to nothing is null, at any depth.
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"workflow_run.event": "schedule"}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"action": "completed"}"#,
        false,
    ),
    // A literal dotted key beats the path, which is what keeps every stored
    // condition's verdict intact.
    (
        "ToolCalled",
        Some(r#"{"a.b": "literal"}"#),
        "ToolCalled",
        r#"{"a.b": "literal", "a": {"b": "nested"}}"#,
        true,
    ),
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"workflow_run.conclusion": {"$nin": ["success", "skipped"]}}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"workflow_run": {"conclusion": "failure"}}"#,
        true,
    ),
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"workflow_run.conclusion": {"$nin": ["success", "skipped"]}}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"workflow_run": {"conclusion": "success"}}"#,
        false,
    ),
    (
        "ToolCalled",
        Some(r#"{"args.command": {"$regex": "cargo test"}}"#),
        "ToolCalled",
        r#"{"name": "run_bash", "args": {"command": "cd r && cargo test --lib"}}"#,
        true,
    ),
    (
        "ToolCalled",
        Some(r#"{"args.command": {"$regex": "^cargo test"}}"#),
        "ToolCalled",
        r#"{"name": "run_bash", "args": {"command": "cd r && cargo test --lib"}}"#,
        false,
    ),
    // `$or` ORs whole conditions and ANDs with its siblings.
    (
        "ChangeProposed",
        Some(r#"{"repo": "r", "$or": [{"file_count": {"$gt": 9}}, {"risk": "high"}]}"#),
        "ChangeProposed",
        r#"{"repo": "r", "file_count": 1, "risk": "high"}"#,
        true,
    ),
    (
        "ChangeProposed",
        Some(r#"{"repo": "r", "$or": [{"file_count": {"$gt": 9}}, {"risk": "high"}]}"#),
        "ChangeProposed",
        r#"{"repo": "other", "file_count": 20, "risk": "high"}"#,
        false,
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

/// Dropped before any subscriber sees it, so neither surface may take it.
#[test]
fn streaming_variants_are_refused_at_both_surfaces() {
    for name in PER_TOKEN_STREAMING_EVENT_TYPES {
        for err in [awaitable(name).unwrap_err(), triggerable(name).unwrap_err()] {
            assert!(
                err.contains(name) && err.contains("per-token streaming"),
                "refusal for {name} should name the variant and why: {err}",
            );
        }
    }
}

/// The one family the two surfaces disagree about. A wait on it self-satisfies,
/// so the wait surface refuses it. A trigger spawns a separate thread, so it may
/// watch the family and the gate never drops it.
#[test]
fn the_event_wait_family_is_refused_for_a_wait_and_accepted_for_a_trigger() {
    for name in EVENT_WAIT_EVENT_TYPES {
        let err = awaitable(name).unwrap_err();
        assert!(
            err.contains(name),
            "refusal for {name} should name the variant: {err}",
        );
        assert_eq!(
            triggerable(name),
            Ok(SubscriptionVerdict::KnownThreadEvent),
            "a trigger may watch {name}",
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
        awaitable("ChangeProposed"),
        Ok(SubscriptionVerdict::KnownThreadEvent),
    );
    // Leading/trailing whitespace is tolerated, matching normalize_list.
    assert_eq!(
        awaitable("  ResponseGenerated "),
        Ok(SubscriptionVerdict::KnownThreadEvent),
    );
}

#[test]
fn an_unknown_name_is_accepted_for_the_caller_to_corroborate() {
    // A workspace-defined domain event nobody has emitted yet. Refusing these
    // would break the most valuable case for the tool.
    assert_eq!(
        awaitable("ReleasePublished"),
        Ok(SubscriptionVerdict::UnknownName),
    );
}

#[test]
fn an_empty_name_is_refused() {
    assert!(awaitable("   ").is_err());
}

// ── persisted system events (ADR 0113) ──────────────────────────────

/// The bug this rule fixes. A backup completion is a durable fact, and nothing
/// accompanies it. Before ADR 0113 the tool refused it and advised waiting on a
/// companion event that does not exist.
#[test]
fn a_persisted_system_event_name_is_awaitable() {
    for name in ["BackupCompleted", "BackupFailed", "NotificationCreated"] {
        assert_eq!(
            awaitable(name),
            Ok(SubscriptionVerdict::KnownSystemEvent),
            "{name} writes an events row, so a wait on it resolves",
        );
    }
}

/// Every reserved name the engine does not persist, swept rather than sampled,
/// so a new transient variant cannot quietly become awaitable.
#[test]
fn every_transient_system_frame_is_refused() {
    use crate::engine::event_bus::SystemEvent;
    let mut refused = 0;
    for name in SystemEvent::RESERVED_TYPE_NAMES {
        if SystemEvent::is_persisted_type_name(name)
            || crate::engine::thread_lifecycle::classify_event(name).is_some()
        {
            continue;
        }
        let Err(err) = awaitable(name) else {
            panic!("{name} writes no row, so it must be refused");
        };
        assert!(err.contains(name), "the refusal must name the frame: {err}");
        refused += 1;
    }
    assert!(refused > 20, "the sweep found only {refused} frames");
}

/// The advice has to point at something that exists. That is what the old
/// message got wrong.
#[test]
fn the_refusal_names_the_terminal_event_where_one_exists() {
    let err = awaitable("BackupProgress").unwrap_err();
    assert!(err.contains("transient"), "{err}");
    assert!(err.contains("BackupCompleted"), "{err}");
    assert!(err.contains("BackupFailed"), "{err}");

    // A frame with no terminal twin simply gets the shorter message.
    let err = awaitable("Toast").unwrap_err();
    assert!(err.contains("transient"), "{err}");
    assert!(!err.contains("Subscribe to"), "nothing to point at: {err}");
}

/// A wrapper is not an event. Calling either one transient would be a lie, and
/// a wait on it could never match: the row is filed under the inner name.
#[test]
fn a_transport_wrapper_is_refused_as_a_wrapper() {
    for name in TRANSPORT_TYPE_NAMES {
        let err = awaitable(name).unwrap_err();
        assert!(err.contains("wrapper"), "{name}: {err}");
        assert!(!err.contains("transient"), "{name}: {err}");
    }
}

/// The gate both fan-outs run, so the set each offers is one function rather
/// than two expressions that happen to agree today (I8).
#[test]
fn the_system_gate_admits_a_persisted_frame_and_any_domain_event() {
    use crate::engine::event_bus::SystemEvent;
    let started = chrono::Utc::now();
    assert!(is_subscribable_system_event(&SystemEvent::BackupFailed {
        error: "disk full".to_string(),
        started_at: started,
        finished_at: started,
    }));
    assert!(!is_subscribable_system_event(
        &SystemEvent::BackupProgress {
            phase: "dumping".to_string(),
            progress: 2,
            total: 5,
        }
    ));

    // A domain event qualifies on either setting of `transient`: the workspace
    // chose the name, so the emitter and the subscriber are the same party.
    for transient in [false, true] {
        assert!(is_subscribable_system_event(&SystemEvent::DomainEvent {
            event_type: "ReleasePublished".to_string(),
            payload: json!({"summary": "v2"}),
            depth: 0,
            transient,
            actor: None,
        }));
    }
}

/// `ChangeDiscarded` is both a `SystemEvent` variant and a `ThreadEvent` one.
/// The thread-scoped reading wins: it is the only one of the pair a `condition`
/// can scope to a thread.
#[test]
fn a_name_that_is_both_resolves_to_the_thread_event() {
    assert_eq!(
        awaitable("ChangeDiscarded"),
        Ok(SubscriptionVerdict::KnownThreadEvent),
    );
}

// ── near misses and retired names ───────────────────────────────────

/// The incident. Both names were accepted, armed, and never matched.
#[test]
fn the_incident_names_are_refused_with_the_right_suggestion() {
    let err = refuse_near_miss("CredentialStored").unwrap_err();
    assert!(
        err.starts_with("CredentialStored is not an event Lucidos emits"),
        "{err}"
    );
    assert!(err.contains("Did you mean CredentialCreated"), "{err}");

    let err = refuse_near_miss("CredentialRequestResolved").unwrap_err();
    assert!(err.contains("Did you mean CredentialRequested"), "{err}");
}

/// A retired name is caught by exact lookup, never by the heuristic. A rename
/// that changed the leading words leaves no letters to match on.
#[test]
fn a_retired_name_is_refused_and_names_its_replacement() {
    for (retired, replacement) in [
        ("ClaudeCodeIdled", "CodingAgentIdled"),
        ("MemorySearched", "MemoryRecalled"),
    ] {
        let err = awaitable(retired).unwrap_err();
        assert!(err.contains("retired"), "{retired}: {err}");
        assert!(err.contains(replacement), "{retired}: {err}");
    }
}

#[test]
fn a_plain_misspelling_of_an_engine_name_is_refused() {
    for (typo, expected) in [
        ("ThreadFinished", "ThreadArchived"),
        ("ResponseGenerted", "ResponseGenerated"),
        ("ChangeAplied", "ChangeApplied"),
        ("ToolCall", "ToolCalled"),
    ] {
        let err = refuse_near_miss(typo).unwrap_err();
        assert!(err.contains(expected), "{typo}: {err}");
    }
}

/// Case 2, the one to get right. A workspace's own domain event passes, even
/// when it reads like an engine name.
#[test]
fn a_workspace_domain_event_is_never_refused_by_the_heuristic() {
    for name in [
        "ReleasePublished",
        "EmailReceived",
        "OrderCreated",
        "InvoicePaid",
        "ReleaseFinnished",
        "StandupPosted",
        "PriceDropped",
        "DeployStarted",
    ] {
        assert!(
            refuse_near_miss(name).is_ok(),
            "{name} belongs to the workspace",
        );
    }
}

#[test]
fn the_never_emitted_note_names_the_type_and_the_way_to_check() {
    let note = never_emitted_warning("ReleaseFinnished");
    assert!(note.contains("ReleaseFinnished"), "{note}");
    assert!(note.contains("event_types"), "{note}");
}

/// The corpus is derived, so a rename cannot strand a name in it. Every entry
/// has to validate as known at the surface it was drawn for. This is the
/// contract the `event_types` action rests on: a name the agent reads off the
/// list is always a name the validator then accepts.
#[test]
fn every_derived_known_name_validates_as_known() {
    for surface in [SubscriptionSurface::Wait, SubscriptionSurface::Trigger] {
        for name in known_names::subscribable_event_type_names(surface) {
            let verdict = validate_subscribable_event_type(name, surface);
            assert!(
                matches!(
                    verdict,
                    Ok(SubscriptionVerdict::KnownThreadEvent
                        | SubscriptionVerdict::KnownSystemEvent)
                ),
                "{name} at {surface:?}: {verdict:?}",
            );
        }
    }
}

/// A retired name must not also be a live one. Revert a rename without
/// dropping its alias and every subscription on the revived name is refused.
#[test]
fn no_retired_alias_is_also_a_live_known_name() {
    let live = known_names::subscribable_event_type_names(SubscriptionSurface::Trigger);
    for alias in ThreadEvent::LEGACY_TYPE_NAME_ALIASES {
        assert!(!live.contains(alias), "{alias} is both retired and live");
    }
}

/// A suggestion the caller cannot act on is worse than none.
#[test]
fn a_suggestion_is_always_a_name_a_wait_can_match() {
    for name in [
        "CredentialStored",
        "ClaudeCodeIdled",
        "BackupProgres",
        "TextStreamd",
    ] {
        for suggestion in known_names::suggest_event_types(name) {
            let verdict = awaitable(suggestion);
            assert!(
                matches!(
                    verdict,
                    Ok(SubscriptionVerdict::KnownThreadEvent
                        | SubscriptionVerdict::KnownSystemEvent)
                ),
                "{name} suggested {suggestion}: {verdict:?}",
            );
        }
    }
}

// ── the adjacent-tag envelope ───────────────────────────────────────

/// `SystemEvent` is `#[serde(tag = "type", content = "data")]`, so its row is
/// `{"type": …, "data": {…}}`. The condition language is a flat one-level
/// lookup, so the view must flatten or every such condition is silently false.
#[test]
fn the_envelope_is_flattened_so_a_condition_names_the_events_own_fields() {
    let row = json!({"type": "BackupFailed", "data": {"error": "disk full"}});
    let view = matchable_payload("BackupFailed", row, None);
    assert_eq!(view, json!({"error": "disk full"}));
    assert!(sub("BackupFailed", Some(json!({"error": "disk full"}))).matches("BackupFailed", &view));
}

/// The live frame and the stored row are the same bytes, built by the same
/// `to_payload`, so one function flattens both.
#[test]
fn the_live_view_of_a_system_event_is_the_view_of_its_row() {
    use crate::engine::event_bus::SystemEvent;
    let started = chrono::Utc::now();
    let event = SystemEvent::BackupFailed {
        error: "disk full".to_string(),
        started_at: started,
        finished_at: started,
    };
    // What the row's payload column holds, replayed through the row path.
    let replayed = matchable_payload(event.stored_event_type(), event.to_payload(), None);
    assert_eq!(matchable_system_payload(&event), replayed);
    assert_eq!(replayed["error"], json!("disk full"));
}

/// A unit-like variant serializes with no `data` at all. No persisted variant
/// is unit-like today, so this pins the arm that keeps the next one matchable.
#[test]
fn a_content_free_envelope_flattens_to_an_empty_object() {
    let row = json!({"type": "BackupFailed"});
    assert_eq!(matchable_payload("BackupFailed", row, None), json!({}));
}

/// Under a persisted name the shape decides, so a payload that merely owns a
/// `type` key keeps every field it wrote.
#[test]
fn a_payload_that_is_not_the_envelope_passes_through() {
    // `type` names something else.
    let p = json!({"type": "pdf", "data": {"x": 1}});
    assert_eq!(matchable_payload("ArtifactImported", p.clone(), None), p);

    // Right `type`, but a third key, so it is the event's own fields.
    let p = json!({"type": "ArtifactImported", "data": {"x": 1}, "summary": "s"});
    assert_eq!(matchable_payload("ArtifactImported", p.clone(), None), p);

    // Right `type`, but the content cannot carry named fields.
    let p = json!({"type": "ArtifactImported", "data": 7});
    assert_eq!(matchable_payload("ArtifactImported", p.clone(), None), p);
}

/// The shape alone is ambiguous, so the name decides first. Only a persisted
/// `SystemEvent` name writes a real envelope, and a workspace may author a
/// domain payload in that exact shape. Flattening it would drop what it wrote.
#[test]
fn a_domain_event_authored_in_the_envelope_shape_keeps_its_payload() {
    let authored = json!({"type": "ReleasePublished", "data": {"version": "1.2.0"}});
    assert_eq!(
        matchable_payload("ReleasePublished", authored.clone(), None),
        authored
    );
    let row = json!({"type": "BackupFailed", "data": {"error": "disk full"}});
    assert_eq!(
        matchable_payload("BackupFailed", row, None),
        json!({"error": "disk full"})
    );
}

// ── matchable_payload ───────────────────────────────────────────────

#[test]
fn the_matchable_payload_carries_the_thread_the_event_belongs_to() {
    let thread = Uuid::new_v4();
    let view = matchable_payload(
        "CodingAgentIdled",
        json!({"has_changes": true}),
        Some(thread),
    );

    assert_eq!(view["thread_id"], json!(thread.to_string()));
    assert!(
        sub(
            "CodingAgentIdled",
            Some(json!({"thread_id": thread.to_string()}))
        )
        .matches("CodingAgentIdled", &view),
        "scoping a subscription to one thread is the whole point of the view"
    );
    assert!(
        !sub(
            "CodingAgentIdled",
            Some(json!({"thread_id": Uuid::new_v4().to_string()})),
        )
        .matches("CodingAgentIdled", &view),
        "and it must not match some other thread's event"
    );
}

/// A `uuid::Uuid` serializes as its hyphenated string, so the injected value has
/// to be that same spelling: a condition carries whatever the `threads` list
/// printed, and `$eq` on a JSON string is a byte comparison.
#[test]
fn the_injected_id_is_spelled_the_way_a_uuid_serializes() {
    let thread = Uuid::new_v4();
    let view = matchable_payload("ThreadArchived", json!({}), Some(thread));
    assert_eq!(view["thread_id"], serde_json::to_value(thread).unwrap());
}

/// Insert-if-absent. An event that owns the key keeps its value, which is what
/// keeps a user-authored domain payload honest.
#[test]
fn an_owned_thread_id_is_never_shadowed() {
    let carrier = Uuid::new_v4();
    let owned = Uuid::new_v4();
    let view = matchable_payload(
        "ToolCalled",
        json!({"thread_id": owned.to_string()}),
        Some(carrier),
    );
    assert_eq!(view["thread_id"], json!(owned.to_string()));
}

/// An event belonging to no thread is not thread-scopable, and the view says so
/// by omission rather than by inventing a value. A condition naming a missing
/// field never matches (`missing_field_does_not_match`), so such a subscription
/// simply never fires, identically on the live and the replay path.
#[test]
fn an_event_with_no_thread_gets_no_key() {
    let payload = json!({"summary": "release published"});
    let view = matchable_payload("ReleasePublished", payload.clone(), None);
    assert_eq!(view, payload);
    assert!(view.get("thread_id").is_none());

    // Which is what makes a domain event not thread-scopable, on every path:
    // the key is absent rather than invented, and a condition naming a missing
    // field never matches (`missing_field_does_not_match`). So such a
    // subscription is silent, identically live and on replay, instead of one
    // path resolving it.
    assert!(
        !sub(
            "ReleasePublished",
            Some(json!({"thread_id": Uuid::new_v4().to_string()})),
        )
        .matches("ReleasePublished", &view),
        "a thread-scoped condition must match nothing on an event with no thread"
    );
}

/// Domain payloads are whatever the workspace wrote, including a bare scalar.
#[test]
fn a_non_object_payload_is_left_alone() {
    let view = matchable_payload(
        "SleepImported",
        json!("just a string"),
        Some(Uuid::new_v4()),
    );
    assert_eq!(view, json!("just a string"));
}

// ── validate_emittable_event_type ────────────────────────────────────

#[test]
fn an_empty_event_type_is_refused() {
    assert!(validate_emittable_event_type("").is_err());
}

/// Spoofing prevention: an untrusted app must not emit a domain event whose
/// name collides with a `SystemEvent` variant. After the SSE unwrap, the wire
/// frame is indistinguishable from a real system frame, and a forged
/// `NotificationCreated` would reach every connected client.
#[test]
fn a_system_event_name_is_refused() {
    for name in [
        "NotificationCreated",
        "NotificationRead",
        "PreferencesChanged",
        "AppDeleted",
        "Toast",
        "DomainEvent",
        "ChangesUpdated",
        "TriggerCreated",
        "ThreadEvent",
    ] {
        assert!(
            validate_emittable_event_type(name).is_err(),
            "{name} should be rejected as reserved",
        );
    }
}

/// Regression: a `ThreadEvent` name is refused too.
///
/// Only `SystemEvent` names were, so one `emit_event("EventWaitStarted", ...)`
/// wrote a permanent domain row whose `aggregate_id` was that literal string.
/// The boot rebuild casts `aggregate_id::uuid`, so the row wedged it. Events
/// are append-only, which makes the fix a boundary and never a cleanup.
#[test]
fn a_thread_event_name_is_refused() {
    for name in [
        "EventWaitStarted",
        "EventWaitDelivered",
        "MessageReceived",
        "ResponseGenerated",
        "ToolCalled",
        "SessionStarted",
        // Transient variants: never persisted as thread rows, but a domain row
        // under the name poisons a future query just the same.
        "NavigationRequested",
        "CumulativeTextUpdated",
        // Legacy serde aliases deserialize INTO their variant, so refusing the
        // new spelling and allowing the old one leaves the forgery standing.
        "Thinking",
        "ClaudeCodeToolCalled",
    ] {
        assert!(
            validate_emittable_event_type(name).is_err(),
            "{name} is a thread-event wire name and must be refused",
        );
    }
}

/// Every name the deny lists carry is refused, with no gap between the list
/// and the predicate that reads it.
#[test]
fn every_reserved_name_is_refused() {
    use crate::engine::event_bus::SystemEvent;
    use crate::engine::thread_events::ThreadEvent;

    for name in SystemEvent::RESERVED_TYPE_NAMES
        .iter()
        .chain(ThreadEvent::RESERVED_TYPE_NAMES)
        .chain(ThreadEvent::LEGACY_TYPE_NAME_ALIASES)
    {
        assert!(
            validate_emittable_event_type(name).is_err(),
            "{name} is on a deny list but the validator accepts it",
        );
    }
}

#[test]
fn a_workspace_domain_name_is_accepted() {
    for name in [
        "SlidePresenterState",
        "SlideRemoteCommand",
        "HabitCompleted",
        "MyCustomEvent",
    ] {
        assert!(
            validate_emittable_event_type(name).is_ok(),
            "{name} should be allowed as a domain event",
        );
    }
}

// ── the catalog ─────────────────────────────────────────────────────

/// Write one row directly, the only way to stand up history no live emit can
/// produce. `EventBus` refuses a retired name and never writes a transient
/// frame, and both shapes sit in a store that has run through a rename.
async fn insert_raw_event(pool: &sqlx::PgPool, event_type: &str) {
    sqlx::query("INSERT INTO events (id, event_type, payload) VALUES ($1, $2, $3)")
        .bind(Uuid::new_v4())
        .bind(event_type)
        .bind(json!({"summary": "a row from before the rename"}))
        .execute(pool)
        .await
        .expect("seed a historical row");
}

/// **Every name the catalog offers is a name the validator accepts.**
///
/// A store carries the names of retired engine events, because the rows
/// outlive the rename. Offering one would send the caller to a dropdown entry
/// their next subscription refuses.
#[tokio::test]
async fn the_catalog_never_offers_a_name_the_validator_refuses() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let store = crate::core::store::EventStore::new(pool.clone());

    let retired = "ClaudeCodeIdled";
    assert!(
        ThreadEvent::LEGACY_TYPE_NAME_ALIASES.contains(&retired),
        "{retired} has to still be a retired name for this test to mean anything",
    );
    insert_raw_event(&pool, retired).await;
    insert_raw_event(&pool, "BackupProgress").await;
    insert_raw_event(&pool, "ReleasePublished").await;

    for surface in [SubscriptionSurface::Wait, SubscriptionSurface::Trigger] {
        let catalog = event_type_catalog(&store, surface)
            .await
            .expect("the store answers");
        assert!(
            catalog.workspace.contains(&"ReleasePublished".to_string()),
            "a domain event is what the workspace half is for: {:?}",
            catalog.workspace,
        );
        assert!(
            !catalog.workspace.contains(&retired.to_string()),
            "{retired} is retired and would be refused: {:?}",
            catalog.workspace,
        );
        for name in catalog
            .engine
            .iter()
            .map(|n| n.to_string())
            .chain(catalog.workspace.iter().cloned())
        {
            assert!(
                validate_subscribable_event_type(&name, surface).is_ok(),
                "{name} is offered at {surface:?} but refused",
            );
        }
    }

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// Waiting on a name and being allowed to emit it are different permissions.
///
/// Persisted system events became awaitable (ADR 0113), and the forge guard is
/// deliberately untouched: every one of them is still refused here.
#[test]
fn a_name_that_became_awaitable_is_still_not_emittable() {
    use crate::engine::event_bus::SystemEvent;
    for name in SystemEvent::PERSISTED_TYPE_NAMES {
        assert!(
            awaitable(name).is_ok(),
            "{name} writes an events row, so a wait on it resolves",
        );
        assert!(
            validate_emittable_event_type(name).is_err(),
            "{name} must stay unforgeable over the emit endpoint",
        );
    }
}
