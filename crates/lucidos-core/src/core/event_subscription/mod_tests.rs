use super::*;
use serde_json::json;

fn sub(event_type: &str, condition: Option<Value>) -> EventSubscription {
    EventSubscription {
        event_type: event_type.to_string(),
        condition,
    }
}

fn awaitable(name: &str) -> Result<SubscriptionVerdict, String> {
    validate_subscribable_event_type(name, SubscriptionSurface::Wait)
}

fn triggerable(name: &str) -> Result<SubscriptionVerdict, String> {
    validate_subscribable_event_type(name, SubscriptionSurface::Trigger)
}

#[test]
fn matches_requires_both_name_and_condition() {
    let s = sub("ChangeProposed", Some(json!({"file_count": {"$gt": 0}})));
    assert!(s.matches("ChangeProposed", &json!({"file_count": 3})));
    assert!(!s.matches("ChangeProposed", &json!({"file_count": 0})));
    assert!(!s.matches("ChangeApplied", &json!({"file_count": 3})));
}

#[test]
fn matches_with_no_condition_is_name_only() {
    let s = sub("ChangeProposed", None);
    assert!(s.matches("ChangeProposed", &json!({"anything": true})));
    assert!(!s.matches("ChangeApplied", &json!({"anything": true})));
}

#[test]
fn renamed_role_events_match_existing_subscriptions_and_stored_rows() {
    let legacy = "WorkItemWatched";
    let current = "RootThreadWatched";
    for (name, alias) in [(legacy, current), (current, legacy)] {
        let subscription = sub(name, Some(json!({"user_id": "example-user"})));
        assert_eq!(
            subscription.event_type_candidates().collect::<Vec<_>>(),
            [name, alias]
        );
        assert!(subscription.matches(alias, &json!({"user_id": "example-user"})));
        assert!(!subscription.matches(alias, &json!({"user_id": "another-user"})));
    }
}

#[test]
fn any_matches_is_per_entry_or() {
    let subs = vec![
        sub("ChangeProposed", Some(json!({"file_count": {"$gt": 5}}))),
        sub("ResponseGenerated", None),
    ];
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

    let obj = json!({"event_type": "X", "condition": null});
    let s = EventSubscription::from_object_entry(obj.as_object().unwrap()).unwrap();
    assert_eq!(s.condition, None);

    assert!(EventSubscription::from_object_entry(json!({}).as_object().unwrap()).is_none());
    assert!(
        EventSubscription::from_object_entry(json!({"event_type": "  "}).as_object().unwrap())
            .is_none()
    );
}

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
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        },
        ThreadEvent::CodingAgentThoughtStreamed {
            text: String::new(),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
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
    assert!(is_subscribable(&ThreadEvent::ToolCalled {
        name: "run_bash".to_string(),
        args: json!({}),
        description: String::new(),
    }));
    assert!(is_subscribable(&ThreadEvent::ThreadArchived));
}

#[test]
fn engine_turn_idles_are_never_subscribable() {
    use crate::engine::thread_events::{CEILING_REENTRY_IDLE_REASON, KEEP_ALIVE_IDLE_REASON};
    let idle = |reason: Option<&str>| ThreadEvent::CodingAgentIdled {
        has_changes: false,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
        reason: reason.map(str::to_string),
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
        turn_end: None,
    };
    assert!(is_subscribable(&idle(None)), "the agent's own idle fires");
    for reason in [KEEP_ALIVE_IDLE_REASON, CEILING_REENTRY_IDLE_REASON] {
        assert!(
            !is_subscribable(&idle(Some(reason))),
            "the gate admitted {reason}"
        );
        assert!(
            crate::engine::thread_events::not_an_engine_turn_idle_sql("")
                .contains(&format!("'{reason}'")),
            "the SQL scans disagree with the predicate on {reason}"
        );
    }
}

#[test]
fn side_questions_are_never_subscribable() {
    let events = crate::test_support::every_side_question_event();
    let mut names: Vec<&str> = events.iter().map(ThreadEvent::event_type).collect();
    names.sort_unstable();
    let mut listed = ThreadEvent::SIDE_QUESTION_EVENT_TYPES.to_vec();
    listed.sort_unstable();
    assert_eq!(names, listed, "the const and the predicate disagree");
    for event in &events {
        let name = event.event_type();
        assert!(event.is_side_question_event(), "{name} fails the predicate");
        assert!(!is_subscribable(event), "the gate admitted {name}");
        for err in [awaitable(name).unwrap_err(), triggerable(name).unwrap_err()] {
            assert!(err.contains(name) && err.contains("side question"), "{err}");
        }
        for surface in [SubscriptionSurface::Wait, SubscriptionSurface::Trigger] {
            assert!(
                !known_names::subscribable_event_type_names(surface).contains(&name),
                "{name} is offered as subscribable"
            );
        }
    }
}

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
    assert_eq!(
        awaitable("  ResponseGenerated "),
        Ok(SubscriptionVerdict::KnownThreadEvent),
    );
}

#[test]
fn an_unknown_name_is_accepted_for_the_caller_to_corroborate() {
    assert_eq!(
        awaitable("ReleasePublished"),
        Ok(SubscriptionVerdict::UnknownName),
    );
}

#[test]
fn an_empty_name_is_refused() {
    assert!(awaitable("   ").is_err());
}

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

#[test]
fn the_refusal_names_the_terminal_event_where_one_exists() {
    let err = awaitable("BackupProgress").unwrap_err();
    assert!(err.contains("transient"), "{err}");
    assert!(err.contains("BackupCompleted"), "{err}");
    assert!(err.contains("BackupFailed"), "{err}");

    let err = awaitable("SnapshotProgress").unwrap_err();
    for name in [
        "SnapshotCreated",
        "SnapshotCaptureFailed",
        "SnapshotChecked",
    ] {
        assert!(err.contains(name), "{err}");
        assert!(awaitable(name).is_ok());
    }
    assert!(awaitable("SnapshotDeleted").is_ok());

    let err = awaitable("Toast").unwrap_err();
    assert!(err.contains("transient"), "{err}");
    assert!(!err.contains("Subscribe to"), "nothing to point at: {err}");
}

#[test]
fn a_transport_wrapper_is_refused_as_a_wrapper() {
    for name in TRANSPORT_TYPE_NAMES {
        let err = awaitable(name).unwrap_err();
        assert!(err.contains("wrapper"), "{name}: {err}");
        assert!(!err.contains("transient"), "{name}: {err}");
    }
}

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

#[test]
fn a_name_that_is_both_resolves_to_the_thread_event() {
    assert_eq!(
        awaitable("ChangeDiscarded"),
        Ok(SubscriptionVerdict::KnownThreadEvent),
    );
}

#[test]
fn the_incident_names_are_refused_with_the_right_suggestion() {
    let err = refuse_near_miss("CredentialStored").unwrap_err();
    assert!(
        err.starts_with("CredentialStored is not an event Lucidos emits"),
        "{err}"
    );
    assert!(err.contains("Did you mean CredentialCreated"), "{err}");

    let err = refuse_near_miss("CredentialRequestResolved").unwrap_err();
    assert!(err.contains("Did you mean FormRequestResolved"), "{err}");
    assert!(err.contains("CredentialRequested"), "{err}");
}

#[test]
fn a_retired_name_is_refused_and_names_its_replacement() {
    for (retired, replacement) in [
        ("ClaudeCodeIdled", "CodingAgentIdled"),
        ("MemorySearched", "MemoryRecalled"),
        ("UserPromptInjected", "PromptInjected"),
    ] {
        for err in [awaitable(retired), triggerable(retired)].map(Result::unwrap_err) {
            assert!(err.contains("retired"), "{retired}: {err}");
            assert!(err.contains(replacement), "{retired}: {err}");
        }
        assert!(
            validate_emittable_event_type(retired).is_err(),
            "{retired} must not be emittable: its rows read back as {replacement}",
        );
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

#[test]
fn no_retired_alias_is_also_a_live_known_name() {
    let live = known_names::subscribable_event_type_names(SubscriptionSurface::Trigger);
    for alias in ThreadEvent::LEGACY_TYPE_NAME_ALIASES {
        assert!(!live.contains(alias), "{alias} is both retired and live");
    }
}

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

#[test]
fn the_envelope_is_flattened_so_a_condition_names_the_events_own_fields() {
    let row = json!({"type": "BackupFailed", "data": {"error": "disk full"}});
    let view = matchable_payload("BackupFailed", row, None);
    assert_eq!(view, json!({"error": "disk full"}));
    assert!(sub("BackupFailed", Some(json!({"error": "disk full"}))).matches("BackupFailed", &view));
}

#[test]
fn the_live_view_of_a_system_event_is_the_view_of_its_row() {
    use crate::engine::event_bus::SystemEvent;
    let started = chrono::Utc::now();
    let event = SystemEvent::BackupFailed {
        error: "disk full".to_string(),
        started_at: started,
        finished_at: started,
    };
    let replayed = matchable_payload(event.stored_event_type(), event.to_payload(), None);
    assert_eq!(matchable_system_payload(&event), replayed);
    assert_eq!(replayed["error"], json!("disk full"));
}

#[test]
fn a_content_free_envelope_flattens_to_an_empty_object() {
    let row = json!({"type": "BackupFailed"});
    assert_eq!(matchable_payload("BackupFailed", row, None), json!({}));
}

#[test]
fn a_payload_that_is_not_the_envelope_passes_through() {
    let p = json!({"type": "pdf", "data": {"x": 1}});
    assert_eq!(matchable_payload("ArtifactImported", p.clone(), None), p);

    let p = json!({"type": "ArtifactImported", "data": {"x": 1}, "summary": "s"});
    assert_eq!(matchable_payload("ArtifactImported", p.clone(), None), p);

    let p = json!({"type": "ArtifactImported", "data": 7});
    assert_eq!(matchable_payload("ArtifactImported", p.clone(), None), p);
}

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

#[test]
fn the_injected_id_is_spelled_the_way_a_uuid_serializes() {
    let thread = Uuid::new_v4();
    let view = matchable_payload("ThreadArchived", json!({}), Some(thread));
    assert_eq!(view["thread_id"], serde_json::to_value(thread).unwrap());
}

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

#[test]
fn an_event_with_no_thread_gets_no_key() {
    let payload = json!({"summary": "release published"});
    let view = matchable_payload("ReleasePublished", payload.clone(), None);
    assert_eq!(view, payload);
    assert!(view.get("thread_id").is_none());

    assert!(
        !sub(
            "ReleasePublished",
            Some(json!({"thread_id": Uuid::new_v4().to_string()})),
        )
        .matches("ReleasePublished", &view),
        "a thread-scoped condition must match nothing on an event with no thread"
    );
}

#[test]
fn a_non_object_payload_is_left_alone() {
    let view = matchable_payload(
        "SleepImported",
        json!("just a string"),
        Some(Uuid::new_v4()),
    );
    assert_eq!(view, json!("just a string"));
}

#[test]
fn an_empty_event_type_is_refused() {
    assert!(validate_emittable_event_type("").is_err());
}

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

#[test]
fn a_route_frame_name_is_refused() {
    for name in crate::core::event_subscription::frames::ROUTE_FRAMES {
        assert!(
            validate_emittable_event_type(name).is_err(),
            "{name} should be rejected as a route frame",
        );
    }
}

#[test]
fn a_thread_event_name_is_refused() {
    for name in [
        "EventWaitStarted",
        "EventWaitDelivered",
        "MessageReceived",
        "ResponseGenerated",
        "ToolCalled",
        "SessionStarted",
        "NavigationRequested",
        "CumulativeTextUpdated",
        "Thinking",
        "ClaudeCodeToolCalled",
    ] {
        assert!(
            validate_emittable_event_type(name).is_err(),
            "{name} is a thread-event wire name and must be refused",
        );
    }
}

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

async fn insert_raw_event(pool: &sqlx::PgPool, event_type: &str) {
    sqlx::query("INSERT INTO events (id, event_type, payload) VALUES ($1, $2, $3)")
        .bind(Uuid::new_v4())
        .bind(event_type)
        .bind(json!({"summary": "a row from before the rename"}))
        .execute(pool)
        .await
        .expect("seed a historical row");
}

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
    insert_raw_event(&pool, "WorkItemWatched").await;
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
        assert!(catalog.engine.contains(&"RootThreadWatched"));
        assert!(!catalog
            .workspace
            .iter()
            .any(|name| name == "WorkItemWatched"));
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
