use super::*;
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, MessageOrigin, SessionEndReason, ThreadEvent,
};

#[test]
fn bus_event_variants_are_constructable() {
    let thread_event = BusEvent::Thread {
        thread_id: Uuid::new_v4(),
        event: ThreadEvent::MessageReceived {
            text: "hello".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    };
    assert!(matches!(thread_event, BusEvent::Thread { .. }));

    // Transient events use the same Thread variant with is_persisted() == false
    let transient = BusEvent::Thread {
        thread_id: Uuid::new_v4(),
        event: ThreadEvent::TextStreaming {
            text: "chunk".into(),
        },
        meta: EventMeta::NONE,
    };
    assert!(matches!(transient, BusEvent::Thread { .. }));

    let system = BusEvent::System(SystemEvent::PreferencesChanged {
        key: "tz".into(),
        value: Some("UTC".into()),
        actor: None,
    });
    assert!(matches!(system, BusEvent::System(_)));
}

#[test]
fn emitted_event_carries_sequence_and_type() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(42),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event: ThreadEvent::ResponseGenerated {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        },
    };
    assert_eq!(emitted.seq, Some(42));
    assert!(matches!(emitted.typed, BusEvent::Thread { .. }));
}

#[test]
fn transient_events_have_no_sequence() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event: ThreadEvent::TextStreaming { text: "hi".into() },
            meta: EventMeta::NONE,
        },
    };
    assert_eq!(emitted.seq, None);
}

#[test]
fn broadcast_channel_works() {
    let (tx, mut rx) = broadcast::channel::<EmittedEvent>(16);

    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event: ThreadEvent::TextStreaming {
                text: "hello".into(),
            },
            meta: EventMeta::NONE,
        },
    };

    let _ = tx.send(emitted);

    match rx.try_recv() {
        Ok(received) => {
            assert_eq!(received.seq, None);
            assert!(matches!(received.typed, BusEvent::Thread { .. }));
        }
        Err(e) => panic!("Expected event, got: {:?}", e),
    }
}

#[test]
fn consumers_can_pattern_match_bus_events() {
    let event = BusEvent::Thread {
        thread_id: Uuid::new_v4(),
        event: ThreadEvent::ToolCalled {
            name: "search".into(),
            args: serde_json::json!({}),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    };

    let is_thread = matches!(&event, BusEvent::Thread { .. });
    assert!(is_thread);

    if let BusEvent::Thread {
        event: ThreadEvent::ToolCalled { name, .. },
        ..
    } = &event
    {
        assert_eq!(name, "search");
    } else {
        panic!("Expected Thread::ToolCalled");
    }
}

// -----------------------------------------------------------------------
// to_sse_json — SSE JSON shape tests
// -----------------------------------------------------------------------

#[test]
fn thread_event_sse_json_has_seq_and_event_id() {
    let tid = Uuid::new_v4();
    let eid = Uuid::new_v4();
    let emitted = EmittedEvent {
        event_id: eid,
        seq: Some(42),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::MessageReceived {
                text: "hello".into(),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta::NONE,
        },
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "ThreadEvent");
    assert_eq!(json["data"]["thread_id"], tid.to_string());
    assert_eq!(json["data"]["seq"], 42);
    assert_eq!(
        json["data"]["event_id"],
        eid.to_string(),
        "SSE JSON must include event_id for frontend pending message matching"
    );
    assert_eq!(json["data"]["event"]["type"], "MessageReceived");
    assert_eq!(json["data"]["event"]["text"], "hello");
}

#[test]
fn thread_event_sse_json_includes_meta_channel() {
    let tid = Uuid::new_v4();
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(1),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::MessageReceived {
                text: "fix bug".into(),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..Default::default()
            },
        },
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(
        json["data"]["event"]["channel"], "claude_code",
        "SSE JSON must include channel from EventMeta so frontend shows correct label"
    );
}

#[test]
fn thread_event_sse_json_omits_channel_when_none() {
    let tid = Uuid::new_v4();
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(1),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::MessageReceived {
                text: "hello".into(),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta::NONE,
        },
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert!(
        json["data"]["event"].get("channel").is_none(),
        "SSE JSON should not include channel when EventMeta has None"
    );
}

#[test]
fn thread_event_sse_json_has_created() {
    let tid = Uuid::new_v4();
    let now = Utc::now();
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(7),
        created: now,
        typed: BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::TextStreamed { text: "hi".into() },
            meta: EventMeta::NONE,
        },
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    let created_str = json["data"]["created"]
        .as_str()
        .expect("created must be present in SSE JSON");
    assert!(
        created_str.contains("T"),
        "created should be an ISO timestamp"
    );
}

#[test]
fn transient_event_sse_json_has_no_seq() {
    let tid = Uuid::new_v4();
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::TextStreaming {
                text: "chunk".into(),
            },
            meta: EventMeta::NONE,
        },
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "ThreadEvent");
    assert_eq!(json["data"]["thread_id"], tid.to_string());
    assert!(
        json["data"]["seq"].is_null(),
        "transient events should not have seq"
    );
    assert_eq!(json["data"]["event"]["type"], "TextStreaming");
}

#[test]
fn system_notification_created_matches_server_event_shape() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::NotificationCreated {
            id: "n-1".into(),
            title: "Test".into(),
            message: "Hello".into(),
            task_id: None,
            app_id: None,
        }),
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "NotificationCreated");
    assert_eq!(json["data"]["id"], "n-1");
    assert_eq!(json["data"]["title"], "Test");
    assert_eq!(json["data"]["message"], "Hello");
}

#[test]
fn system_preferences_changed_matches_server_event_shape() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::PreferencesChanged {
            key: "timezone".into(),
            value: Some("Europe/Oslo".into()),
            actor: None,
        }),
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "PreferencesChanged");
    assert_eq!(json["data"]["key"], "timezone");
    assert_eq!(json["data"]["value"], "Europe/Oslo");
}

#[test]
fn system_changes_updated_matches_server_event_shape() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::ChangesUpdated {
            pending: vec![],
            applied: vec![],
            total_pending: 0,
            restart_required: false,
        }),
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "ChangesUpdated");
    assert_eq!(json["data"]["total_pending"], 0);
    assert_eq!(json["data"]["restart_required"], false);
}

#[test]
fn system_memory_rebuild_progress_matches_server_event_shape() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::MemoryRebuildProgress {
            processed: 50,
            total: 100,
            percent: 50,
        }),
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "MemoryRebuildProgress");
    assert_eq!(json["data"]["percent"], 50);
}

#[test]
fn system_backup_progress_matches_server_event_shape() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::BackupProgress {
            phase: "uploading".into(),
            progress: 3,
            total: 10,
        }),
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "BackupProgress");
    assert_eq!(json["data"]["phase"], "uploading");
}

/// Regression test: domain events emitted via `lucidos.events.emit()` must
/// be broadcast on SSE with the inner event_type as the wire `type`, so
/// that frontend listeners (`lucidos.sse.on('SlidePresenterState', ...)`)
/// fire. Otherwise the SDK only sees the wrapper `"DomainEvent"` and
/// per-type subscribers never get called — breaking app-to-app comms
/// (e.g. Super Slides presenter ↔ remote).
#[test]
fn domain_event_sse_json_uses_inner_event_type() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(99),
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::DomainEvent {
            event_type: "SlidePresenterState".into(),
            payload: serde_json::json!({"slide_index": 3, "is_paused": false}),
            depth: 0,
            transient: false,
        }),
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(
        json["type"], "SlidePresenterState",
        "Wire `type` must be the inner event_type, not the wrapper 'DomainEvent', \
             so frontend `lucidos.sse.on('SlidePresenterState', ...)` fires"
    );
    assert_eq!(
        json["data"]["slide_index"], 3,
        "Wire `data` must be the raw payload, not {{event_type, payload}}"
    );
    assert_eq!(json["data"]["is_paused"], false);
    assert!(
        json["data"].get("event_type").is_none(),
        "Wire `data` must not contain the wrapper's event_type field"
    );
    assert!(
        json["data"].get("payload").is_none(),
        "Wire `data` must not contain the wrapper's payload field"
    );
}

/// Drift guard: every `SystemEvent` variant's `event_type()` must appear
/// in `RESERVED_TYPE_NAMES`. If a new variant is added but missed here,
/// the `emit_event` HTTP guard would silently let untrusted apps spoof
/// it on the SSE wire.
#[test]
fn reserved_type_names_match_event_type() {
    use SystemEvent::*;
    let samples = vec![
        NotificationCreated {
            id: "x".into(),
            title: "t".into(),
            message: "m".into(),
            task_id: None,
            app_id: None,
        },
        NotificationRead {
            id: "x".into(),
            actor: None,
        },
        NotificationsAllRead { actor: None },
        PreferencesChanged {
            key: "k".into(),
            value: None,
            actor: None,
        },
        MemoryRebuildProgress {
            processed: 0,
            total: 0,
            percent: 0,
        },
        ChangesUpdated {
            pending: vec![],
            applied: vec![],
            total_pending: 0,
            restart_required: false,
        },
        BackupProgress {
            phase: "p".into(),
            progress: 0,
            total: 0,
        },
        RecoveryProgress {
            completed: 0,
            total: 0,
        },
        Toast {
            message: "m".into(),
            level: "l".into(),
        },
        ArtifactImported {
            artifact_path: "p".into(),
            source_type: "s".into(),
            source_detail: "d".into(),
            commit_hash: "c".into(),
            summary: None,
        },
        TriggerCreated {
            trigger_id: "t".into(),
            payload: serde_json::json!({}),
            actor: None,
        },
        TriggerUpdated {
            trigger_id: "t".into(),
            payload: serde_json::json!({}),
            actor: None,
        },
        TriggerDeleted {
            trigger_id: "t".into(),
            payload: serde_json::json!({}),
            actor: None,
        },
        TriggerEnabled {
            trigger_id: "t".into(),
            payload: serde_json::json!({}),
            actor: None,
        },
        TriggerDisabled {
            trigger_id: "t".into(),
            payload: serde_json::json!({}),
            actor: None,
        },
        TriggerExecuted {
            trigger_id: "t".into(),
            payload: serde_json::json!({}),
        },
        AppCreated {
            app_id: "a".into(),
            name: None,
            actor: None,
        },
        AppUpdated {
            app_id: "a".into(),
            name: None,
            actor: None,
        },
        AppDeleted {
            app_id: "a".into(),
            actor: None,
        },
        DomainEvent {
            event_type: "X".into(),
            payload: serde_json::json!({}),
            depth: 0,
            transient: false,
        },
        ArtifactCreated {
            artifact_path: "p".into(),
            commit: "c".into(),
            source: None,
        },
        ArtifactUpdated {
            artifact_path: "p".into(),
            commit: "c".into(),
            source: None,
        },
        ArtifactDeleted {
            artifact_path: "p".into(),
            commit: "c".into(),
        },
        LanguageSet {
            language: "en".into(),
        },
        TimezoneSet {
            timezone: "Europe/Oslo".into(),
        },
        RepositoryImported {
            url: "u".into(),
            branch: "b".into(),
            destination: "d".into(),
            file_count: 0,
            skipped_count: 0,
            commit: "c".into(),
            files: vec![],
        },
        TriggerCompleted {
            trigger_id: "t".into(),
            trigger_name: "n".into(),
            result_summary: "r".into(),
        },
        ChangeDiscarded {
            change_id: "c".into(),
        },
        ThreadFocused {
            thread_id: Uuid::nil(),
            device_id: "d".into(),
        },
        ThreadUnfocused {
            thread_id: Uuid::nil(),
            device_id: "d".into(),
        },
    ];
    for ev in &samples {
        let name = ev.event_type();
        assert!(SystemEvent::is_reserved_type_name(name),
                "SystemEvent::{} missing from RESERVED_TYPE_NAMES — emit_event HTTP guard would let apps spoof this frame",
                name);
    }
    assert!(
        SystemEvent::is_reserved_type_name("ThreadEvent"),
        "ThreadEvent wrapper must be reserved so apps cannot forge thread-event frames"
    );
}

#[test]
fn notification_created_is_persisted() {
    let event = SystemEvent::NotificationCreated {
        id: "n-1".into(),
        title: "Test".into(),
        message: "Hello".into(),
        task_id: Some("t-1".into()),
        app_id: Some("morning-brief".into()),
    };
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "notification");
    assert_eq!(event.aggregate_id(), "n-1");
}

#[test]
fn notification_created_skips_none_fields_in_sse() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::NotificationCreated {
            id: "n-2".into(),
            title: "Alert".into(),
            message: "Something happened".into(),
            task_id: None,
            app_id: None,
        }),
    };
    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert!(
        json["data"].get("task_id").is_none(),
        "None task_id should be skipped"
    );
    assert!(
        json["data"].get("app_id").is_none(),
        "None app_id should be skipped"
    );
}

#[test]
fn preferences_changed_is_persisted_with_key_value() {
    let set = SystemEvent::PreferencesChanged {
        key: "language".into(),
        value: Some("nb".into()),
        actor: None,
    };
    assert!(set.is_persisted());
    assert_eq!(set.aggregate(), "preference");
    assert_eq!(set.event_type(), "PreferencesChanged");

    // to_payload() wraps in serde's tag/content envelope: { "type": "...", "data": { ... } }
    let json = set.to_payload();
    assert_eq!(json["data"]["key"], "language");
    assert_eq!(json["data"]["value"], "nb");
}

#[test]
fn preferences_changed_delete_has_null_value() {
    let del = SystemEvent::PreferencesChanged {
        key: "old_setting".into(),
        value: None,
        actor: None,
    };
    assert!(del.is_persisted());

    let json = del.to_payload();
    assert_eq!(json["data"]["key"], "old_setting");
    // value is None → skipped by skip_serializing_if
    assert!(
        json["data"].get("value").is_none(),
        "None value should be skipped"
    );
}

#[test]
fn other_system_events_not_persisted() {
    assert!(!SystemEvent::NotificationRead {
        id: "n-1".into(),
        actor: None
    }
    .is_persisted());
    assert!(!SystemEvent::NotificationsAllRead { actor: None }.is_persisted());
    assert!(!SystemEvent::MemoryRebuildProgress {
        processed: 0,
        total: 0,
        percent: 0
    }
    .is_persisted());
    assert!(!SystemEvent::Toast {
        message: "hi".into(),
        level: "info".into()
    }
    .is_persisted());
}

#[test]
fn trigger_created_serializes_for_sse() {
    let event = SystemEvent::TriggerCreated {
        trigger_id: "abc".to_string(),
        payload: serde_json::json!({"name": "Test"}),
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "TriggerCreated");
    assert_eq!(json["data"]["trigger_id"], "abc");
    assert!(event.is_persisted());
}

#[test]
fn app_created_serializes_for_sse() {
    let event = SystemEvent::AppCreated {
        app_id: "my-app".to_string(),
        name: Some("My App".to_string()),
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "AppCreated");
    assert_eq!(json["data"]["app_id"], "my-app");
    assert!(!event.is_persisted());
}

#[test]
fn app_deleted_serializes_for_sse() {
    let event = SystemEvent::AppDeleted {
        app_id: "habit-tracker".to_string(),
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "AppDeleted");
    assert!(!event.is_persisted());
}

#[test]
fn artifact_created_serializes_and_persists() {
    let event = SystemEvent::ArtifactCreated {
        artifact_path: "notes/todo.md".to_string(),
        commit: "abc1234".to_string(),
        source: Some("run_python".to_string()),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ArtifactCreated");
    assert_eq!(json["data"]["artifact_path"], "notes/todo.md");
    assert_eq!(json["data"]["commit"], "abc1234");
    assert_eq!(json["data"]["source"], "run_python");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "artifact");
    assert_eq!(event.aggregate_id(), "notes/todo.md");
}

#[test]
fn artifact_updated_skips_none_source() {
    let event = SystemEvent::ArtifactUpdated {
        artifact_path: "report.md".to_string(),
        commit: "def5678".to_string(),
        source: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ArtifactUpdated");
    assert!(
        json["data"].get("source").is_none(),
        "None source should be skipped"
    );
    assert!(event.is_persisted());
}

#[test]
fn artifact_deleted_serializes_and_persists() {
    let event = SystemEvent::ArtifactDeleted {
        artifact_path: "old.txt".to_string(),
        commit: "aaa1111".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ArtifactDeleted");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "artifact");
}

#[test]
fn language_set_serializes_and_persists() {
    let event = SystemEvent::LanguageSet {
        language: "nb".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "LanguageSet");
    assert_eq!(json["data"]["language"], "nb");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "preference");
}

#[test]
fn timezone_set_serializes_and_persists() {
    let event = SystemEvent::TimezoneSet {
        timezone: "Europe/Oslo".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "TimezoneSet");
    assert_eq!(json["data"]["timezone"], "Europe/Oslo");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "preference");
}

#[test]
fn repository_imported_serializes_and_persists() {
    let event = SystemEvent::RepositoryImported {
        url: "https://github.com/user/repo".to_string(),
        branch: "main".to_string(),
        destination: "repo".to_string(),
        file_count: 42,
        skipped_count: 3,
        commit: "bbb2222".to_string(),
        files: vec!["README.md".to_string(), "src/main.rs".to_string()],
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "RepositoryImported");
    assert_eq!(json["data"]["file_count"], 42);
    assert_eq!(json["data"]["files"].as_array().unwrap().len(), 2);
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "artifact");
    assert_eq!(event.aggregate_id(), "repo");
}

#[test]
fn trigger_completed_serializes_and_persists() {
    let event = SystemEvent::TriggerCompleted {
        trigger_id: "task-123".to_string(),
        trigger_name: "Daily backup".to_string(),
        result_summary: "Completed successfully".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "TriggerCompleted");
    assert_eq!(json["data"]["trigger_id"], "task-123");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "trigger");
    assert_eq!(event.aggregate_id(), "task-123");
}

#[test]
fn change_discarded_serializes_and_persists() {
    let event = SystemEvent::ChangeDiscarded {
        change_id: "c-456".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ChangeDiscarded");
    assert_eq!(json["data"]["change_id"], "c-456");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "change");
    assert_eq!(event.aggregate_id(), "c-456");
}

#[test]
fn domain_event_is_persisted_with_inner_payload() {
    let event = SystemEvent::DomainEvent {
        event_type: "SlideTextEdited".to_string(),
        payload: serde_json::json!({"slide_id": 1, "summary": "Updated title"}),
        depth: 0,
        transient: false,
    };
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "domain");
    assert_eq!(event.aggregate_id(), "SlideTextEdited");
    // Must not double-wrap — DB and SSE consumers expect the raw inner payload
    let payload = event.to_payload();
    assert_eq!(payload["slide_id"], 1);
    assert_eq!(payload["summary"], "Updated title");
}

/// Transient domain events (e.g. SlidePresenterState heartbeats every 3s)
/// must be broadcast on SSE but skip the events table — otherwise we'd
/// write ~1,200 rows/hour for a single Super Slides session.
#[test]
fn transient_domain_event_is_not_persisted() {
    let event = SystemEvent::DomainEvent {
        event_type: "SlidePresenterState".to_string(),
        payload: serde_json::json!({"slide_index": 5}),
        depth: 0,
        transient: true,
    };
    assert!(
        !event.is_persisted(),
        "transient domain events must not be written to the events table"
    );
}

/// Transient flag is internal control state — must not appear on the SSE
/// wire format (frontend listeners shouldn't have to know about it).
#[test]
fn transient_domain_event_sse_json_uses_inner_event_type() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::DomainEvent {
            event_type: "SlidePresenterPing".into(),
            payload: serde_json::json!({"timestamp": 1_700_000_000}),
            depth: 0,
            transient: true,
        }),
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(
        json["type"], "SlidePresenterPing",
        "Wire `type` must be the inner event_type, even for transient events"
    );
    assert_eq!(json["data"]["timestamp"], 1_700_000_000);
    assert!(
        json["data"].get("transient").is_none(),
        "The transient flag is internal — it must not leak to the SSE wire format"
    );
}

/// Phase 4: per-turn idle is signaled by `CodingAgentIdled`, NOT
/// `SessionEnded`. ChangeProposed must appear before the idle marker so the
/// frontend can render "Done · 1 change" the moment idle fires. SessionEnded
/// is now reserved for terminal events (Shutdown / Panic / Closed) and never
/// appears in the normal turn sequence.
#[test]
fn coding_agent_idled_is_last_event_after_change_proposed() {
    let (tx, mut rx) = broadcast::channel::<EmittedEvent>(16);
    let tid = Uuid::new_v4();

    // Correct event ordering for a CC turn that proposes a change:
    // ResponseGenerated → ChangeProposed → CodingAgentIdled
    let events = vec![
        ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        ThreadEvent::ChangeProposed {
            change_id: "c-1".into(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            path: String::new(),
            diff: String::new(),
        },
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
    ];

    for event in &events {
        let _ = tx.send(EmittedEvent {
            event_id: Uuid::new_v4(),
            seq: Some(0), // seq doesn't matter for ordering test
            created: Utc::now(),
            typed: BusEvent::Thread {
                thread_id: tid,
                event: event.clone(),
                meta: EventMeta::NONE,
            },
        });
    }

    let mut received = vec![];
    while let Ok(e) = rx.try_recv() {
        if let BusEvent::Thread { event, .. } = &e.typed {
            received.push(event.event_type().to_string());
        }
    }

    assert_eq!(
        received,
        vec!["ResponseGenerated", "ChangeProposed", "CodingAgentIdled"]
    );

    assert_eq!(
        received.last().unwrap(),
        "CodingAgentIdled",
        "CodingAgentIdled must be the final event — ChangeProposed must come before it"
    );
}

use crate::test_support::{setup_test_db, teardown_test_db};

/// Create a parent Chat thread and a child thread, returning (parent_id, child_id).
async fn spawn_parent_child(bus: &EventBus, child_channel: EventChannel) -> (Uuid, Uuid) {
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "do something".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "child task".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(child_channel),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    (parent_id, child_id)
}

async fn assert_active_children(pool: &PgPool, parent_id: Uuid, expected: i32, msg: &str) {
    let count: i32 = sqlx::query_scalar(
        "SELECT active_children_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, expected, "{}", msg);
}

#[tokio::test]
async fn test_fan_out_parent_callback() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "do three things".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Three children with parent_thread_id
    for (i, &cid) in child_ids.iter().enumerate() {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                text: format!("task {}", i + 1),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: Some(parent_id),
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    // Verify projection — all 3 children have correct parent_thread_id
    let children: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT thread_id FROM thread_summaries WHERE parent_thread_id = $1 ORDER BY thread_id",
    )
    .bind(parent_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(children.len(), 3, "should have 3 children in projection");

    // Verify parent has no parent_thread_id
    let parent_parent: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(
        parent_parent,
        Some(None),
        "parent thread should have no parent_thread_id"
    );

    // Children complete — emit ResponseGenerated THEN CodingAgentIdled for each
    // (mirrors real CC flow). Only CodingAgentIdled should trigger the callback
    // because these are CC threads (source = 'claude_code').
    for &cid in &child_ids {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::ResponseGenerated {
                text: "Done.".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: None,
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    // Verify exactly 3 callbacks (not 6) — ResponseGenerated is skipped for CC threads
    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        3,
        "should have 3 parent callbacks, not 6 (no double-reporting)"
    );
    for cb in &callbacks {
        assert_eq!(cb.parent_thread_id, parent_id);
        assert!(
            cb.callback_text.contains("completed"),
            "callback_text should contain 'completed', got: {}",
            cb.callback_text
        );
    }

    // Verify each callback references a different child
    let mut callback_children: Vec<Uuid> = callbacks.iter().map(|cb| cb.child_thread_id).collect();
    callback_children.sort();
    let mut expected_children = child_ids.clone();
    expected_children.sort();
    assert_eq!(
        callback_children, expected_children,
        "each callback should reference a different child thread"
    );

    // Cleanup
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Three chat children all complete with ResponseGenerated.
/// Verify all 3 callbacks are received (no lost callbacks).
#[tokio::test]
async fn test_fan_out_chat_children_all_report_back() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "research crypto sectors".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Three chat children with parent_thread_id
    for (i, &cid) in child_ids.iter().enumerate() {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                text: format!("research sector {}", i + 1),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: Some(parent_id),
                spawning_event_id: None,
                mode: ActorMode::Agent,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    // Verify parent has 3 active children
    assert_active_children(&pool, parent_id, 3, "parent should have 3 active children").await;

    // Children complete one by one with ResponseGenerated
    for (i, &cid) in child_ids.iter().enumerate() {
        // Emit a ResponseGenerated for the child (this is its response text)
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::ResponseGenerated {
                text: format!("Sector {} analysis complete.", i + 1),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    // Verify all 3 callbacks were received
    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        3,
        "all 3 chat children should report back via callback, got {}",
        callbacks.len()
    );

    // Verify each callback references a different child
    let mut callback_children: Vec<Uuid> = callbacks.iter().map(|cb| cb.child_thread_id).collect();
    callback_children.sort();
    let mut expected_children = child_ids.clone();
    expected_children.sort();
    assert_eq!(
        callback_children, expected_children,
        "each callback should reference a different child thread"
    );

    // Verify parent's active_children_count is 0 after all children completed
    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should have 0 active children after all completed",
    )
    .await;

    // Cleanup
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC children that end via SessionEnded without ever emitting CodingAgentIdled
/// (e.g., crash, shutdown, user-ended) should still send a callback to the parent
/// and decrement active_children_count.
#[tokio::test]
async fn test_cc_child_session_ended_without_idle_sends_callback() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "research something".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // CC child with parent_thread_id
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "do subtask".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Mark child as CC via SessionStarted
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            session_id: "cc-session-1".into(),
            branch: String::new(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    // CC child ends via SessionEnded WITHOUT ever emitting CodingAgentIdled
    // (simulates crash/shutdown/user-ended scenario). Phase 4 leaves only
    // terminal-only reasons; Panic stands in for the prior `Completed` here.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Panic,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Verify callback was received
    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        1,
        "CC child ending via SessionEnded (no prior idle) should send callback, got {}",
        callbacks.len()
    );
    assert_eq!(callbacks[0].child_thread_id, child_id);
    assert_eq!(callbacks[0].parent_thread_id, parent_id);

    // Verify parent's active_children_count decremented to 0
    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should have 0 active children after SessionEnded",
    )
    .await;

    // Cleanup
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC children that DO emit CodingAgentIdled should NOT send a duplicate callback
/// when SessionEnded fires afterward.
#[tokio::test]
async fn test_cc_child_no_duplicate_callback_after_idle_then_session_ended() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "research something".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // CC child with parent_thread_id
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "do subtask".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Mark child as CC
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            session_id: "cc-session-2".into(),
            branch: String::new(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    // CC child idles normally (this sends callback + decrements)
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(&pool, parent_id, 0, "parent should have 0 after idle").await;

    // Drain the callback from CodingAgentIdled
    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        1,
        "should have exactly 1 callback from idle"
    );

    // Now SessionEnded fires (terminal: thread closed/shutdown after idle)
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Should NOT get a second callback
    let mut extra_callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        extra_callbacks.push(cb);
    }
    assert_eq!(
        extra_callbacks.len(),
        0,
        "SessionEnded after CodingAgentIdled should NOT send duplicate callback, got {}",
        extra_callbacks.len()
    );

    // active_children_count should still be 0 (not -1)
    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should still have 0 after SessionEnded",
    )
    .await;

    // Cleanup
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_session_started_updates_source_in_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Thread initially created as "chat" (e.g. by spawn_thread)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix something".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Verify source is "chat"
    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        source.as_deref(),
        Some("chat"),
        "initial source should be chat"
    );

    // SessionStarted fires when CC starts — should update source to "claude_code"
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "test-session".into(),
            branch: String::new(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Verify source is now "claude_code"
    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        source.as_deref(),
        Some("claude_code"),
        "source should be updated to claude_code after SessionStarted"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_session_started_does_not_update_last_activity() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Create thread with MessageReceived
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix it".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Record last_activity after MessageReceived
    let activity_after_msg: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_activity FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    // Small delay so timestamps differ
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // SessionStarted should NOT update last_activity
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "test-session".into(),
            branch: String::new(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let activity_after_session: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_activity FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(
            activity_after_msg, activity_after_session,
            "SessionStarted should not update last_activity — it's a technical event, not user activity"
        );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Phase 4: SessionEnded is terminal-only. Every reason (Shutdown, Panic,
/// Closed, plus the LegacyNonTerminal catch-all for old DB rows) must
/// transition the thread to a terminal status with `has_response = TRUE`.
/// The legacy `StaleResume` carve-out is gone — the dispatcher (Phase 5) owns
/// the retry path; SessionEnded is no longer issued mid-turn.
#[tokio::test]
async fn test_session_ended_transitions_to_terminal_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix it".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "running");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Panic,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "SessionEnded {{ Panic }} must transition to terminal idle status"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC permission prompt must move the thread out of 'running' into
/// 'waiting_for_user_answer' so the drawer surfaces it in REVIEW with the
/// question-mark badge. CodingAgentPermissionResolved returns it to 'running'
/// so the in-flight CC tool call can resume.
#[tokio::test]
async fn test_permission_request_transitions_status_to_waiting_for_user_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "edit my skill".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "sess-1".into(),
            branch: "claude-code/branch".into(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "running");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-1".into(),
            tool_use_id: "tu-1".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({"file_path": "/tmp/x"}),
            summary: "Edit /tmp/x".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "waiting_for_user_answer",
        "CodingAgentPermissionRequest must surface the thread in REVIEW"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-1".into(),
            allowed: true,
            reason: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "running",
        "CodingAgentPermissionResolved must return the thread to running so CC can resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_session_started_stores_repo_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let repo_uuid = "550e8400-e29b-41d4-a716-446655440000";

    // Create thread with MessageReceived
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "analyze this repo".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // SessionStarted with repo_id should store it in thread_summaries
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
            repo_id: Some(repo_uuid.into()),
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let stored_repo_id: Option<String> =
        sqlx::query_scalar("SELECT cc_repo_id FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored_repo_id.as_deref(),
        Some(repo_uuid),
        "cc_repo_id must be stored from SessionStarted"
    );

    // A subsequent SessionStarted WITHOUT repo_id must NOT clear the stored repo_id
    // (this is the follow-up scenario where the frontend doesn't send repo_id)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "s2".into(),
            branch: "claude-code/followup".into(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let stored_repo_id_after: Option<String> =
        sqlx::query_scalar("SELECT cc_repo_id FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_repo_id_after.as_deref(), Some(repo_uuid),
            "cc_repo_id must persist across sessions — follow-up SessionStarted with no repo_id must not clear it");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_non_cc_child_callbacks_on_response_generated() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "parent task".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Non-CC child thread (source = "chat")
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "child task".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Non-CC child completes with ResponseGenerated — should trigger callback
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Result.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        1,
        "non-CC child should callback on ResponseGenerated"
    );
    assert_eq!(callbacks[0].parent_thread_id, parent_id);
    assert_eq!(callbacks[0].child_thread_id, child_id);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_active_children_count_on_child_spawn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, _child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_active_children(
        &pool,
        parent_id,
        1,
        "parent should have 1 active child after spawn",
    )
    .await;

    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "default",
        "parent section stays default; Ongoing is display-only"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_active_children_decremented_on_canceled_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseCanceled {
            text: "partial".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must be 0 after child canceled",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_active_children_decremented_on_aborted_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must be 0 after child aborted",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_cc_child_session_ended_without_idle_decrements_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::CodingAgent).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    // CC session starts then is canceled immediately — no CodingAgentIdled emitted
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            session_id: "test-session".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseCanceled {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // SessionEnded must decrement since no CodingAgentIdled was emitted.
    // Phase 4: only terminal-only reasons (Shutdown / Panic / Closed) remain;
    // Closed stands in for the prior `UserEnded` here.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Closed,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must be 0 after CC child ended without idle",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_chat_parent_stays_default_on_child_spawn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Create parent as a Chat thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "do something".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Verify parent starts with section = 'default'
    let status: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "default", "chat parent should start as 'default'");

    // Spawn child thread with parent_thread_id
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "child task".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Chat parents stay 'default' — they show as Ongoing via deriveThreadStatus
    // (has active children).
    let status: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "default",
        "chat parent should stay 'default' after child spawn"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_section_unread_on_child_complete() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Create parent as Chat thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "parent task".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Spawn non-CC child
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "child task".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Chat parent stays 'default' (not 'waiting') — contract rejects waiting for Chat
    let status: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "default",
        "chat parent stays default while child runs"
    );

    // Child completes — notify_parent_if_child marks parent as 'unread'
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Verify parent is now 'unread' (child completion marks parent unread via callback)
    let status: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "unread",
        "parent should be 'unread' after child completes"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_section_marked_read() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Create a regular chat thread (not scheduled task)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "hello".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Complete it — chat threads become 'unread' on ResponseGenerated
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Reply.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "unread",
        "thread should be 'unread' after completion"
    );

    // Mark as read
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadMarkedRead,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "default",
        "should be 'default' after marking as read"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_threads_skip_review() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Create thread on the trigger channel
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("daily".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Complete it — scheduled tasks should NOT go to 'unread' (no user watching)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Report done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "default",
        "scheduled task should stay 'default', not appear in review"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_followup_response_goes_to_review() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Step 1: Scheduled trigger creates the thread
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("daily".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Step 2: Scheduled task completes — stays in default (history)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Report done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "default",
        "initial scheduled task response should stay in history"
    );

    // Step 3: User sends a followup message on this thread
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "Can you elaborate on the report?".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Step 4: LLM responds to the followup — should go to review (unread)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Here are the details...".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "unread",
        "followup response on scheduled task thread should go to review"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn message_received_destructures_parent_thread_id() {
    // Verify that MessageReceived with parent_thread_id is correctly
    // destructured in the projection match arm (compile-time check)
    // and that the field is accessible for SQL binding.
    let parent_id = Uuid::new_v4();
    let thread_id = Uuid::new_v4();

    let event = ThreadEvent::MessageReceived {
        text: "fan-out message".into(),
        images: vec![],
        device_id: None,
        device: None,
        image_description: None,
        parent_thread_id: Some(parent_id),
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    };

    // Verify destructuring matches the projection handler pattern
    if let ThreadEvent::MessageReceived {
        text,
        parent_thread_id,
        ..
    } = &event
    {
        assert_eq!(text, "fan-out message");
        assert_eq!(*parent_thread_id, Some(parent_id));
    } else {
        panic!("Expected MessageReceived");
    }

    // Verify SSE JSON includes parent_thread_id when present
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(1),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id,
            event: event.clone(),
            meta: EventMeta::NONE,
        },
    };
    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(
        json["data"]["event"]["parent_thread_id"],
        parent_id.to_string()
    );

    // Verify None parent_thread_id doesn't appear in SSE JSON
    let event_no_parent = ThreadEvent::MessageReceived {
        text: "follow-up".into(),
        images: vec![],
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
    let emitted_no_parent = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(2),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id,
            event: event_no_parent,
            meta: EventMeta::NONE,
        },
    };
    let json2: serde_json::Value = serde_json::from_str(&emitted_no_parent.to_sse_json()).unwrap();
    assert!(json2["data"]["event"].get("parent_thread_id").is_none()
            || json2["data"]["event"]["parent_thread_id"].is_null(),
            "None parent_thread_id should be absent or null — follow-up messages must not clear it in projection");
}

// --- Recursion guard tests ---

/// Helper: emit a MessageReceived event for a thread with an optional parent.
async fn emit_thread_message(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>, text: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: text.into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: parent,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn test_recursion_guard_allows_shallow_threads() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root task").await;

    // Spawning a child from root (depth 0 → child depth 1) should succeed
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, root).await;
    assert!(result.is_ok(), "depth 0→1 should be allowed");
    assert_eq!(result.unwrap(), 1);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_enforces_max_depth() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // Build a chain of threads at increasing depths
    let mut chain: Vec<Uuid> = Vec::new();
    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;
    chain.push(root);

    // Build chain up to MAX_THREAD_DEPTH
    for i in 1..=crate::engine::chat::MAX_THREAD_DEPTH {
        let child = Uuid::new_v4();
        emit_thread_message(
            &bus,
            child,
            Some(*chain.last().unwrap()),
            &format!("child {}", i),
        )
        .await;
        chain.push(child);
    }

    // Last thread in chain is at MAX_THREAD_DEPTH — spawning from it should fail
    let deepest = *chain.last().unwrap();
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, deepest).await;
    assert!(result.is_err(), "spawning beyond max depth should fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("Maximum thread nesting depth"),
        "error should mention depth limit: {}",
        err
    );

    // But spawning from the second-to-last should still succeed
    let second_to_last = chain[chain.len() - 2];
    let result2 =
        crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, second_to_last).await;
    assert!(
        result2.is_ok(),
        "spawning at exactly max depth should succeed"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_depth_stored_in_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    let child = Uuid::new_v4();
    let grandchild = Uuid::new_v4();

    emit_thread_message(&bus, root, None, "root").await;
    emit_thread_message(&bus, child, Some(root), "child").await;
    emit_thread_message(&bus, grandchild, Some(child), "grandchild").await;

    // Verify depths in thread_summaries
    let root_depth: i32 =
        sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
            .bind(root)
            .fetch_one(&pool)
            .await
            .unwrap();
    let child_depth: i32 =
        sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
            .bind(child)
            .fetch_one(&pool)
            .await
            .unwrap();
    let grandchild_depth: i32 =
        sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
            .bind(grandchild)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(root_depth, 0, "root thread should be depth 0");
    assert_eq!(child_depth, 1, "child thread should be depth 1");
    assert_eq!(grandchild_depth, 2, "grandchild thread should be depth 2");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_unknown_parent_treated_as_root() {
    let (pool, db_name) = setup_test_db().await;

    // Check guard for a thread_id that doesn't exist in thread_summaries
    let unknown = Uuid::new_v4();
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, unknown).await;
    assert!(result.is_ok(), "unknown parent should default to depth 0");
    assert_eq!(result.unwrap(), 1);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_graceful_error_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // Build a chain at max depth
    let mut parent = Uuid::new_v4();
    emit_thread_message(&bus, parent, None, "root").await;
    for i in 1..=crate::engine::chat::MAX_THREAD_DEPTH {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(parent), &format!("level {}", i)).await;
        parent = child;
    }

    // Try to spawn from the deepest — should get clear error
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, parent).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("Cannot spawn further child threads"),
        "error should guide the LLM: {}",
        err
    );
    assert!(
        err.contains("complete the task in this thread"),
        "error should suggest alternative: {}",
        err
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_parallel_children_within_limit() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;

    // Spawn children up to the limit — all should succeed
    for i in 0..crate::engine::chat::MAX_CHILDREN_PER_THREAD {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(root), &format!("child {}", i)).await;

        // Each child should be allowed to spawn its own children
        let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, child).await;
        assert!(result.is_ok(), "child {}'s children should be allowed", i);
        assert_eq!(result.unwrap(), 2);
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_enforces_max_children() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;

    // Fill up the children limit
    for i in 0..crate::engine::chat::MAX_CHILDREN_PER_THREAD {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(root), &format!("child {}", i)).await;
    }

    // Now trying to spawn from root should fail — max children reached
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, root).await;
    assert!(result.is_err(), "should reject when max children reached");
    let err = result.unwrap_err();
    assert!(
        err.contains("Maximum child threads per parent"),
        "error should mention children limit: {}",
        err
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_children_limit_per_parent_not_global() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;

    // Fill root's children limit
    let mut first_child = Uuid::new_v4();
    for i in 0..crate::engine::chat::MAX_CHILDREN_PER_THREAD {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(root), &format!("root child {}", i)).await;
        if i == 0 {
            first_child = child;
        }
    }

    // Root is full, but first_child should still be able to spawn its own children
    let result =
        crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, first_child).await;
    assert!(
        result.is_ok(),
        "child should have its own independent children budget"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn response_aborted_marks_chat_thread_unread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // User sends message
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix the bug".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Verify starts as default
    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "default");

    // ResponseAborted (engine crash recovery)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: "This response was interrupted by an engine restart.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Verify thread is now unread
    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "unread",
        "ResponseAborted should mark chat thread as unread"
    );

    // Verify has_response is true
    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "ResponseAborted should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn response_canceled_sets_has_response_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // User sends message — creates thread_summaries row with has_response=false
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix the bug".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!has_response, "has_response should start as false");

    // User cancels the response
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: "partial".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // has_response must be true — a canceled response is still a response.
    // Without this, the thread won't appear in get_recent_threads (which
    // filters has_response=TRUE) and becomes invisible after page reload.
    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "ResponseCanceled should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_completed_sets_has_response_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // TriggerStarted creates the thread_summaries row
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("finn-jobb".into()),
            prompt: Some("Check jobs".into()),
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !has_response,
        "has_response should start as false after TriggerStarted"
    );

    // TriggerCompleted — the trigger ran and finished
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerCompleted {
            trigger_id: "t-1".into(),
            trigger_name: Some("finn-jobb".into()),
            result_summary: Some("Found 3 jobs".into()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // has_response must be true — a completed trigger run should appear in
    // get_recent_threads (which filters has_response=TRUE).
    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "TriggerCompleted should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn claude_code_idled_sets_has_response_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "").await;

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !has_response,
        "has_response should start as false after SessionStarted"
    );

    emit_cc_idle(&bus, thread_id, false, None).await;

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "CodingAgentIdled should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn session_ended_sets_has_response_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "").await;

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!has_response, "has_response should start as false");

    // Session ends terminally (e.g. shutdown, panic, or user closed thread).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "SessionEnded (terminal) should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Verify that events with valid request_event_id are accepted without warnings,
/// and that the validation path runs for events with request_event_id set.
#[tokio::test]
async fn test_valid_request_event_id_accepted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Emit a MessageReceived event (this will be the origin)
    let origin = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "fix this".into(),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap();

    // Emit ResponseGenerated with valid request_event_id pointing to the origin
    let response = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseGenerated {
                text: "done".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta {
                request_event_id: Some(origin.event_id),
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap();

    // Verify both events are in DB
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "both events should be persisted");

    // Verify the response event has the correct request_event_id
    let req_id: Option<String> =
        sqlx::query_scalar("SELECT payload->>'request_event_id' FROM events WHERE id = $1")
            .bind(response.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        req_id.as_deref(),
        Some(&origin.event_id.to_string()[..]),
        "response event should reference the origin event"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Verify that automated CC prompts create a valid origin event chain:
/// CodingAgentPromptSent is persisted and can be referenced by subsequent events.
#[tokio::test]
async fn test_automated_prompt_creates_valid_origin() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // First create the thread with a MessageReceived (required for thread_summaries)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "initial".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Emit CodingAgentPromptSent (simulating emit_automated_prompt)
    let prompt_result = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentPromptSent {
                text: "Run /harden now.".into(),
                agent: crate::runtime::AgentKind::ClaudeCode,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap();

    // Now emit ResponseGenerated referencing the prompt
    let response = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseGenerated {
                text: "hardened".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta {
                request_event_id: Some(prompt_result.event_id),
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap();

    // Verify the prompt event exists in DB
    let prompt_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id = $1)")
            .bind(prompt_result.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(prompt_exists, "CodingAgentPromptSent must be persisted");

    // Verify the response references the prompt
    let req_id: Option<String> =
        sqlx::query_scalar("SELECT payload->>'request_event_id' FROM events WHERE id = $1")
            .bind(response.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        req_id.as_deref(),
        Some(&prompt_result.event_id.to_string()[..]),
        "ResponseGenerated should reference CodingAgentPromptSent"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An empty `CodingAgentPromptSent` carries no agent intent and must not
/// flip thread status back to `running`. Real prompts always carry text
/// (user follow-up audit trail, automated CC sessions for hardening /
/// recovery / conflict resolution).
#[tokio::test]
async fn empty_coding_agent_prompt_sent_does_not_flip_status_to_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "do the thing".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "did the thing".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid-empty-prompt".into()),
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status_after_idle: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_after_idle, "idle",
        "sanity: idle with no changes leaves thread idle",
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPromptSent {
            text: String::new(),
            agent: crate::runtime::AgentKind::ClaudeCode,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status_after_empty_prompt: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_after_empty_prompt, "idle",
        "an empty CodingAgentPromptSent carries no user/agent intent — it must \
         not flip status back to 'running'. Real prompts (audit trail for user \
         follow-ups, automated CC sessions) always carry text."
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// After CC exits idle, a follow-up message can auto-resume via the persisted
/// cc_session_id in the CodingAgentIdled event.
#[tokio::test]
async fn cc_follow_up_after_exit_resumes_via_db() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // 1. CC session begins
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // 2. CC finishes work, goes idle with a cc_session_id
    let cc_session_id = "test-session-abc123".to_string();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some(cc_session_id.clone()),
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // 3. Thread status should be 'waiting'
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "waiting");

    // 4. cc_session_id is recoverable from events (auto-resume detection query)
    let recovered: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
             ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recovered, Some(cc_session_id.clone()));

    // 5. Follow-up message arrives (what chat_submit does)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "follow-up message".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // 6. Thread should be 'running' after follow-up
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "running");

    // 7. Auto-resume query would find the session ID (CodingAgentIdled is most recent lifecycle event)
    let q = format!(
        "SELECT event_type, payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type IN ({}) \
             ORDER BY sequence DESC LIMIT 1",
        crate::engine::agent_session::CC_TURN_CLOSER_EVENTS,
    );
    let (event_type, sid): (String, Option<String>) = sqlx::query_as(&q)
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_type, "CodingAgentIdled");
    assert_eq!(sid.as_deref(), Some("test-session-abc123"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// cc_session_id persisted in CodingAgentIdled events survives a simulated engine restart.
#[tokio::test]
async fn cc_session_id_survives_engine_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let cc_session_id = "persistent-session-xyz".to_string();

    // Session starts, does work, goes idle
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some(cc_session_id.clone()),
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // "Restart" — create a new EventBus against the same DB
    let (_bus2, _callback_rx2) = EventBus::new(pool.clone());

    // cc_session_id is still recoverable
    let q = format!(
        "SELECT event_type, payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type IN ({}) \
             ORDER BY sequence DESC LIMIT 1",
        crate::engine::agent_session::CC_TURN_CLOSER_EVENTS,
    );
    let (event_type, sid): (String, Option<String>) = sqlx::query_as(&q)
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_type, "CodingAgentIdled");
    assert_eq!(sid.as_deref(), Some("persistent-session-xyz"));

    // Thread status survived
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "idle"); // CodingAgentIdled with has_changes=false → idle

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Helper: emit SessionStarted on ClaudeCode channel to create a CC thread.
async fn start_cc_session(bus: &EventBus, thread_id: Uuid, branch: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "s1".into(),
            branch: branch.into(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// Helper: emit CodingAgentIdled with the given flags.
async fn emit_cc_idle(
    bus: &EventBus,
    thread_id: Uuid,
    has_changes: bool,
    cc_session_id: Option<&str>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: cc_session_id.map(Into::into),
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// Regression: ChangeApplied followed by CodingAgentIdled(has_changes=false) must leave
/// the thread in 'idle', not 'waiting'. Previously CodingAgentIdled unconditionally set
/// status='waiting', so the reset_worktree_and_idle emission after apply would override
/// the idle status from ChangeApplied — leaving the thread stuck on restart.
#[tokio::test]
async fn change_applied_then_idle_no_changes_stays_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix").await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    let (status, has_changes): (String, bool) =
        sqlx::query_as("SELECT status, cc_has_changes FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "waiting", "CodingAgentIdled with changes → waiting");
    assert!(has_changes);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: "c1".into(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, has_changes): (String, bool) =
        sqlx::query_as("SELECT status, cc_has_changes FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "idle", "ChangeApplied → idle");
    assert!(!has_changes, "ChangeApplied clears cc_has_changes");

    // reset_worktree_and_idle emits CodingAgentIdled { has_changes: false }
    // THIS IS THE REGRESSION SCENARIO: previously this set status back to 'waiting'
    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let (status, has_changes): (String, bool) =
        sqlx::query_as("SELECT status, cc_has_changes FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "CodingAgentIdled(no changes) after ChangeApplied must stay idle"
    );
    assert!(!has_changes, "cc_has_changes stays false");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// After ChangeApplied, the thread stays 'unread' so the Done button appears.
/// CC flags are cleared, so resolveActions returns ['done'] instead of ['apply','discard'].
/// A subsequent CodingAgentIdled(no changes) is idempotent — section stays 'unread'.
#[tokio::test]
async fn change_applied_stays_unread_shows_done() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix").await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    // After CodingAgentIdled(has_changes=true), section should be 'unread'
    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "unread", "CodingAgentIdled with changes → unread");

    // Propose and apply a change
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: "c1".into(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // ChangeApplied does NOT change section — thread stays 'unread' for Done button
    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "unread",
        "ChangeApplied must NOT clear unread — Done button needs to appear"
    );

    // CC flags should be cleared (ClearAll)
    let cc_has_changes: bool =
        sqlx::query_scalar("SELECT cc_has_changes FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!cc_has_changes, "ChangeApplied must clear cc_has_changes");

    // A subsequent CodingAgentIdled(no changes) keeps section as 'unread' (idempotent)
    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "unread",
        "CodingAgentIdled(no changes) keeps section unread"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ChangeDiscarded also keeps the thread unread so Done button appears.
/// Mirror of change_applied_stays_unread_shows_done for the discard path.
#[tokio::test]
async fn change_discarded_stays_unread_shows_done() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix").await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id: "c1".into(),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (section, cc_has_changes): (String, bool) =
        sqlx::query_as("SELECT section, cc_has_changes FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "unread",
        "ChangeDiscarded must NOT clear unread — Done button needs to appear"
    );
    assert!(!cc_has_changes, "ChangeDiscarded must clear cc_has_changes");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Full Apply → Done flow: Apply keeps thread unread, Dismiss moves to default (HISTORY).
#[tokio::test]
async fn apply_then_dismiss_moves_to_history() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix").await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Apply
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: "c1".into(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "unread",
        "After Apply: stays unread for Done button"
    );

    // Done (dismiss)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDismissed,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (section, status): (String, String) =
        sqlx::query_as("SELECT section, status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "default", "After Done: moved to HISTORY");
    assert_eq!(status, "idle", "After Done: status is idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Engine restart with an unresolved CodingAgentPermissionRequest in the
/// event log: `recover_orphan_cc_permission_requests` must emit a paired
/// CodingAgentPermissionResolved so the PermissionCard transitions out of
/// its pending state. Without this fix, the in-memory waiter for the dead
/// CC subprocess is gone and clicking Allow/Deny in the UI 404s forever.
#[tokio::test]
async fn startup_resolves_orphan_permission_request() {
    use crate::engine::agent_recovery::recover_orphan_cc_permission_requests;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/orphan").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-orphan".into(),
            tool_use_id: "tu-orphan".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({"file_path": "/tmp/x"}),
            summary: "Edit /tmp/x".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Pre-restart state: thread is held in waiting_for_user_answer, no Resolved persisted.
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "waiting_for_user_answer",
        "PermissionRequest must put the thread in waiting_for_user_answer"
    );

    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'CodingAgentPermissionResolved' \
           AND payload->>'request_id' = 'req-orphan'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(resolved_count, 0, "no Resolved exists pre-recovery");

    // Simulate engine restart recovery.
    recover_orphan_cc_permission_requests(&pool, &bus).await;

    // Post-recovery: a Resolved event was emitted, projection moved status off
    // waiting_for_user_answer (to 'running'; main.rs's running→idle reset
    // settles it from there in production, but that step is out of scope here).
    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'CodingAgentPermissionResolved' \
           AND payload->>'request_id' = 'req-orphan'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolved_count, 1,
        "recovery must emit exactly one Resolved per orphan request"
    );

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "running",
        "Resolved projection must clear the waiting_for_user_answer status"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Already-resolved permission requests must NOT be re-resolved on startup.
/// Otherwise restart amplifies the Resolved log with duplicate events.
#[tokio::test]
async fn startup_skips_already_resolved_permission_requests() {
    use crate::engine::agent_recovery::recover_orphan_cc_permission_requests;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/already-resolved").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-done".into(),
            tool_use_id: "tu-done".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-done".into(),
            allowed: true,
            reason: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    recover_orphan_cc_permission_requests(&pool, &bus).await;

    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'CodingAgentPermissionResolved' \
           AND payload->>'request_id' = 'req-done'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolved_count, 1,
        "recovery must not duplicate an already-resolved request"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Simulates an engine restart with orphaned waiting threads.
/// Threads in 'waiting' with cc_has_changes=false (dead sessions) should be reset to 'idle'.
/// Threads in 'waiting' with cc_has_changes=true (pending changes) must NOT be reset.
#[tokio::test]
async fn startup_resets_orphaned_waiting_threads_without_changes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Thread A: CC session ended with no changes — simulate pre-fix stuck state
    let thread_a = Uuid::new_v4();
    start_cc_session(&bus, thread_a, "claude-code/a").await;
    emit_cc_idle(&bus, thread_a, false, None).await;

    // Force thread A into 'waiting' to simulate the pre-fix bug where
    // CodingAgentIdled unconditionally set status='waiting'
    sqlx::query("UPDATE thread_summaries SET status = 'waiting' WHERE thread_id = $1")
        .bind(thread_a)
        .execute(&pool)
        .await
        .unwrap();

    // Thread B: CC session with pending changes — should stay waiting
    let thread_b = Uuid::new_v4();
    start_cc_session(&bus, thread_b, "claude-code/b").await;
    bus.emit(BusEvent::Thread {
        thread_id: thread_b,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: true,
            cc_session_id: None,
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Verify pre-restart state
    let status_a: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_a, "waiting",
        "Thread A forced to waiting (simulated pre-fix bug)"
    );

    let status_b: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_b)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status_b, "waiting");

    // Simulate engine restart: run the orphan cleanup query from main.rs
    sqlx::query(
        "UPDATE thread_summaries SET status = 'idle', \
             cc_has_changes = FALSE, cc_requires_restart = FALSE, \
             cc_is_external_repo = FALSE, cc_applying = FALSE \
             WHERE status = 'waiting' AND cc_has_changes = FALSE AND source = 'claude_code'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Thread A: was stuck in waiting with no changes — startup query fixes it
    let status_a: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_a, "idle",
        "Startup must reset orphaned waiting thread to idle"
    );

    // Thread B: has pending changes — must stay waiting
    let (status_b, has_changes_b, requires_restart_b): (String, bool, bool) = sqlx::query_as(
            "SELECT status, cc_has_changes, cc_requires_restart FROM thread_summaries WHERE thread_id = $1"
        ).bind(thread_b).fetch_one(&pool).await.unwrap();
    assert_eq!(
        status_b, "waiting",
        "Thread with pending changes must stay waiting"
    );
    assert!(has_changes_b, "cc_has_changes preserved");
    assert!(requires_restart_b, "cc_requires_restart preserved");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ThreadDismissed must clear all CC flags and set status to idle.
/// Previously ThreadDismissed was a no-op, leaving dismissed threads stuck in waiting.
#[tokio::test]
async fn thread_dismissed_clears_cc_flags_and_goes_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/feat").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: true,
            requires_restart: true,
            cc_session_id: None,
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, has_changes): (String, bool) =
        sqlx::query_as("SELECT status, cc_has_changes FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "waiting");
    assert!(has_changes);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDismissed,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, has_changes, requires_restart, is_external, applying): (
        String,
        bool,
        bool,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT status, cc_has_changes, cc_requires_restart, cc_is_external_repo, cc_applying \
             FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle", "ThreadDismissed must set idle");
    assert!(!has_changes, "ThreadDismissed must clear cc_has_changes");
    assert!(
        !requires_restart,
        "ThreadDismissed must clear cc_requires_restart"
    );
    assert!(
        !is_external,
        "ThreadDismissed must clear cc_is_external_repo"
    );
    assert!(!applying, "ThreadDismissed must clear cc_applying");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Done click invariant: ThreadDismissed must emit LAST, after any trailing
/// CodingAgentIdled from CC cleanup, or the lifecycle side effect re-marks the
/// thread unread and the dismiss is silently undone.
#[tokio::test]
async fn cc_idled_then_dismissed_ends_in_default_section() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/dismiss-order").await;
    emit_cc_idle(&bus, thread_id, false, None).await;
    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "unread", "CodingAgentIdled must mark CC thread unread");

    emit_cc_idle(&bus, thread_id, false, None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDismissed,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "default",
        "ThreadDismissed emitted LAST must leave section=default — Done click would otherwise be silently undone by trailing CodingAgentIdled"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Bug-pinning counter-test: if this ever asserts `default` instead of `unread`,
/// the lifecycle stopped re-marking on CodingAgentIdled and dismiss_thread can
/// drop its ordering hack.
#[tokio::test]
async fn dismissed_then_cc_idled_undoes_dismissal() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/wrong-order").await;
    emit_cc_idle(&bus, thread_id, false, None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDismissed,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    emit_cc_idle(&bus, thread_id, false, None).await;

    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "unread",
        "trailing CodingAgentIdled re-marks the thread unread — this is why dismiss_thread must end CC FIRST"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CodingAgentIdled with has_changes=true must set status to 'waiting' even when
/// cc_has_changes was previously false (first idle after session start).
#[tokio::test]
async fn claude_code_idled_with_changes_sets_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/test").await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    let (status, has_changes): (String, bool) =
        sqlx::query_as("SELECT status, cc_has_changes FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "waiting",
        "First CodingAgentIdled with has_changes=true must set waiting"
    );
    assert!(has_changes);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// External repo CC threads must never get ChangeProposed. The runtime skips
/// propose_change for external repos, so this test verifies the invariant:
/// CodingAgentIdled(has_changes=true, is_external_repo=true) sets the flags
/// correctly, but no changes row exists — meaning Apply/Discard won't appear.
#[tokio::test]
async fn external_repo_idle_with_changes_never_shows_apply_discard() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/external").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: true,
            requires_restart: false,
            cc_session_id: Some("sid-ext".into()),
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, has_changes, is_external): (String, bool, bool) = sqlx::query_as(
            "SELECT status, cc_has_changes, cc_is_external_repo FROM thread_summaries WHERE thread_id = $1"
        ).bind(thread_id).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "waiting");
    assert!(has_changes, "cc_has_changes reflects the payload");
    assert!(is_external, "cc_is_external_repo reflects the payload");

    // The key invariant: no ChangeProposed event should exist for external repos.
    // The runtime skips propose_change, so no changes row is created.
    // Without a pending change, resolve_actions returns [Done], not [Apply, Discard].
    let change_proposed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        change_proposed_count, 0,
        "External repo threads must never have ChangeProposed events — \
             the runtime must skip propose_change for external repos"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Full apply cycle: idle(changes) → apply → idle(no changes) → must end idle.
/// Simulates the exact sequence from apply_now_success: emit_change_applied then
/// reset_worktree_and_idle. The thread must not get stuck in 'waiting'.
#[tokio::test]
async fn full_apply_cycle_ends_idle_not_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let events = vec![
        (
            ThreadEvent::SessionStarted {
                session_id: "s1".into(),
                branch: "claude-code/fix".into(),
                repo_id: None,
            },
            EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        ),
        (
            ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sid".into()),
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::ChangeProposed {
                change_id: "c1".into(),
                description: Some("Fix".into()),
                files: vec!["f.rs".into()],
                requires_restart: false,
                origin: None,
                commit_sha: None,
                branch_name: String::new(),
                repo_root: String::new(),
                hardened: false,
                path: String::new(),
                diff: String::new(),
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::ChangeApplied {
                change_id: "c1".into(),
                requires_restart: false,
                client_update: false,
                commits: vec![],
                thread_title: None,
                actor: None,
                pre_merge_sha: None,
                post_merge_sha: None,
                path: String::new(),
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sid".into()),
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
            },
            EventMeta::NONE,
        ),
    ];

    for (event, meta) in events {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta,
        })
        .await
        .unwrap();
    }

    let (status, has_changes, requires_restart, is_external, applying): (
        String,
        bool,
        bool,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT status, cc_has_changes, cc_requires_restart, cc_is_external_repo, cc_applying \
             FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "After full apply cycle, thread must be idle"
    );
    assert!(!has_changes, "cc_has_changes must be false after apply");
    assert!(!requires_restart);
    assert!(!is_external);
    assert!(!applying);

    let sid: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
             ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        sid.as_deref(),
        Some("sid"),
        "cc_session_id must survive for resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a `ChangeApplied` emitted with `actor: Some(MessageOrigin::Device)`
/// must persist that actor verbatim into the events table. The frontend reads
/// `payload->'actor'` to render the chip ("You" / "<device label>") — when the
/// stored payload is missing the actor or has it as `null`, the
/// `actorInitiator` fallback in `thread-events.ts` collapses to "Lucidos
/// Engine", which is the user-visible bug behind Task 4 of the
/// mode-driven-actor-chip plan. This test pins the lower-level contract that
/// the EventBus persistence pipeline preserves the actor field, so that when
/// the call sites in `apply_change` / `end_stale_waiting_session` /
/// `spawn_hardening_session` are correctly wired, the user actor reaches the
/// frontend.
#[tokio::test]
async fn change_applied_persists_device_actor_in_payload() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let device = MessageOrigin::Device {
        device_id: "dev-actor-test".into(),
        label: "Kenneth's MacBook".into(),
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: "change-actor-test".into(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: Some(device.clone()),
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit ChangeApplied");

    let actor_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplied' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("query persisted ChangeApplied actor");

    let actor_json = actor_json.expect(
        "ChangeApplied payload must carry a non-null `actor` field — \
         a missing/null actor renders as 'Lucidos Engine' in the UI",
    );
    assert_eq!(
        actor_json.get("kind").and_then(|v| v.as_str()),
        Some("device"),
        "actor.kind must be 'device' (not engine/agent), got: {actor_json:?}"
    );
    assert_eq!(
        actor_json.get("device_id").and_then(|v| v.as_str()),
        Some("dev-actor-test"),
        "actor.device_id must round-trip, got: {actor_json:?}"
    );
    assert_eq!(
        actor_json.get("label").and_then(|v| v.as_str()),
        Some("Kenneth's MacBook"),
        "actor.label must round-trip so the chip renders the user's device name, \
         got: {actor_json:?}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Tier 3 slow-path regression: when `apply_change` hands the merge off to a
/// fresh CC subprocess, the user's actor is parked in `pending_apply_actors`
/// keyed by `change_id`. The cleanup in `agent_session::run_session` takes it
/// back out and stamps it on the resulting `ChangeApplied`. This test exercises
/// the stash → take → emit chain end-to-end (DB roundtrip), without spawning
/// CC, and asserts the persisted event carries the device — guarding against
/// any regression that drops the actor across the async gap.
#[tokio::test]
async fn slow_path_change_applied_carries_stashed_apply_actor() {
    use crate::engine::pending_apply_actors::PendingApplyActors;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let stash = PendingApplyActors::default();
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    let device = MessageOrigin::Device {
        device_id: "iphone-slow-path".into(),
        label: "iOS Safari PWA".into(),
    };

    // Apply call site stashes the actor by change_id before spawning CC for the merge.
    stash.stash(change_id, device.clone());

    // Cleanup site (post-merge) takes it back out and stamps it on ChangeApplied.
    let recovered = stash.take(change_id);
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: change_id.to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: recovered,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit ChangeApplied");

    let actor_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplied' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("query persisted ChangeApplied actor");

    let actor_json = actor_json.expect(
        "slow-path ChangeApplied must carry the stashed actor — \
         a missing actor means the stash → take wiring regressed and the chip falls back to 'Lucidos Engine'",
    );
    assert_eq!(
        actor_json.get("kind").and_then(|v| v.as_str()),
        Some("device"),
    );
    assert_eq!(
        actor_json.get("device_id").and_then(|v| v.as_str()),
        Some("iphone-slow-path"),
    );

    // Take is one-shot: a second cleanup pass (e.g. retried apply) sees None
    // and falls back to the engine attribution rather than double-stamping.
    assert!(
        stash.take(change_id).is_none(),
        "stash entry must be consumed after first take"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---- Initiator propagation tests ----

/// User-initiated chat thread has initiator='user'.
#[tokio::test]
async fn initiator_user_chat() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: tid,
        event: ThreadEvent::MessageReceived {
            text: "hello".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(tid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(initiator, "user");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Scheduled task thread has initiator='system'.
#[tokio::test]
async fn initiator_system_trigger() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: tid,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("daily".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(tid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(initiator, "system");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC sub-thread of a scheduled task inherits initiator='system'.
#[tokio::test]
async fn initiator_inherited_system_to_cc_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent: trigger run (system-initiated)
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("e2e tests".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Child: CC thread spawned by trigger run
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "run e2e tests".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let (child_init, child_source): (String, String) =
        sqlx::query_as("SELECT initiator, source FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        child_init, "system",
        "CC child of scheduled task must inherit system initiator"
    );
    assert_eq!(
        child_source, "claude_code",
        "CC child should have claude_code source"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC sub-thread of a user chat has initiator='user'.
#[tokio::test]
async fn initiator_inherited_user_to_cc_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent: user chat
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "help me".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Child: CC thread spawned by agentic loop
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "fix the bug".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let child_init: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        child_init, "user",
        "CC child of user chat inherits user initiator"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// MessageReceived with source="system" creates system-initiated thread.
#[tokio::test]
async fn initiator_from_message_source_field() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: tid,
        event: ThreadEvent::MessageReceived {
            text: "system message".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(tid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(initiator, "system");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// SessionStarted ON CONFLICT does not overwrite initiator set by prior MessageReceived.
#[tokio::test]
async fn initiator_preserved_on_session_started_upsert() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let cc_thread_id = Uuid::new_v4();

    // Parent: trigger run (system)
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("nightly".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // CC child: MessageReceived creates row with initiator=system (inherited)
    bus.emit(BusEvent::Thread {
        thread_id: cc_thread_id,
        event: ThreadEvent::MessageReceived {
            text: "run tests".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // SessionStarted upserts the same row — must not reset initiator
    bus.emit(BusEvent::Thread {
        thread_id: cc_thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "s-1".into(),
            branch: String::new(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(cc_thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        initiator, "system",
        "SessionStarted must not overwrite initiator"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// MessageReceived persists `spawning_event_id` into thread_summaries so a
/// system-spawned thread records the exact parent event that triggered it.
#[tokio::test]
async fn spawning_event_id_persists_for_system_spawn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    let spawn_event = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "spawn a child".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "do work".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: Some(spawn_event),
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let stored: Option<Uuid> =
        sqlx::query_scalar("SELECT spawning_event_id FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        Some(spawn_event),
        "spawning_event_id must be persisted"
    );

    let parent_stored: Option<Uuid> =
        sqlx::query_scalar("SELECT spawning_event_id FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        parent_stored, None,
        "user-initiated thread must have NULL spawning_event_id"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ── Phase 0: Pre-refactor safety net ──

/// Cross-validate: verify that SystemEvent serialization round-trips correctly
/// for all persisted system event variants. If serialization breaks, events
/// written to the DB can't be read back on replay.
#[test]
fn persisted_system_events_round_trip_through_json() {
    let events = vec![
        SystemEvent::NotificationCreated {
            id: "n-1".into(),
            title: "Test".into(),
            message: "Hello".into(),
            task_id: Some("t-1".into()),
            app_id: Some("morning-brief".into()),
        },
        SystemEvent::PreferencesChanged {
            key: "timezone".into(),
            value: Some("Europe/Oslo".into()),
            actor: None,
        },
    ];
    for event in &events {
        assert!(event.is_persisted(), "{:?} should be persisted", event);
        let payload = event.to_payload();
        // Verify payload has the expected envelope structure
        assert!(
            payload.get("type").is_some(),
            "Serialized payload for {:?} must have 'type' field",
            event
        );
        assert!(
            payload.get("data").is_some(),
            "Serialized payload for {:?} must have 'data' field",
            event
        );
    }
}

/// Bug: CodingAgentIdled(has_changes=false) from Default section was suppressed,
/// leaving the thread in HISTORY. First idle (no changes) must surface in REVIEW
/// so the user knows the CC session completed.
#[tokio::test]
async fn cc_idle_no_changes_from_default_goes_to_review() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/explore").await;

    // CC completes with no file changes
    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let (section, status): (String, String) =
        sqlx::query_as("SELECT section, status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "unread",
            "CodingAgentIdled(no changes) from Default must set section to unread (REVIEW), not stay default (HISTORY)");
    assert_eq!(status, "idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Housekeeping: CodingAgentIdled(has_changes=false) when section is already 'unread'
/// (after apply/discard) must not change section — it's already in REVIEW.
#[tokio::test]
async fn cc_idle_no_changes_from_unread_stays_unread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix").await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    // Apply the change — section stays 'unread' for Done button
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: "c1".into(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Housekeeping idle — section already 'unread', must stay 'unread'
    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let section: String =
        sqlx::query_scalar("SELECT section FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "unread",
        "Housekeeping CodingAgentIdled(no changes) must keep section unread"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// When a parent chat thread has active CC children and ResponseGenerated fires,
/// a ChildrenCountChanged event must be broadcast AFTER the ThreadMarkedUnread
/// side effect. This ensures the frontend updates both section and children count
/// atomically, preventing a transient REVIEW state (should be WAITING).
#[tokio::test]
async fn response_generated_on_parent_with_children_broadcasts_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Create parent chat thread
    let parent_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "dispatch work".into(),
            images: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Spawn two CC children
    for i in 0..2 {
        let child_id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::MessageReceived {
                text: format!("child task {}", i),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: Some(parent_id),
                spawning_event_id: None,
                mode: ActorMode::Agent,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    assert_active_children(&pool, parent_id, 2, "parent should have 2 active children").await;

    // Subscribe to capture events AFTER children are spawned
    let mut rx = bus.subscribe();

    // Parent finishes responding → ResponseGenerated → ThreadMarkedUnread side effect
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ResponseGenerated {
            text: "I've started two CC sessions.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Collect all events emitted for the parent thread
    let mut parent_events: Vec<String> = Vec::new();
    let mut saw_marked_unread = false;
    let mut saw_children_count_after_unread = false;

    // Drain all available events (emit is synchronous relative to this test)
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        {
            if *thread_id == parent_id {
                let event_type = event.event_type().to_string();
                if event_type == "ThreadMarkedUnread" {
                    saw_marked_unread = true;
                }
                if event_type == "ChildrenCountChanged" && saw_marked_unread {
                    if let ThreadEvent::ChildrenCountChanged { active, total } = event {
                        assert_eq!(
                            *active, 2,
                            "ChildrenCountChanged must reflect 2 active children"
                        );
                        assert_eq!(
                            *total, 2,
                            "ChildrenCountChanged must reflect 2 total children"
                        );
                    }
                    saw_children_count_after_unread = true;
                }
                parent_events.push(event_type);
            }
        }
    }

    assert!(
        saw_marked_unread,
        "ThreadMarkedUnread must be emitted as side effect of ResponseGenerated on chat thread"
    );
    assert!(
        saw_children_count_after_unread,
        "ChildrenCountChanged must be broadcast AFTER ThreadMarkedUnread to ensure \
             frontend has consistent section + children data. Got events: {:?}",
        parent_events,
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// -----------------------------------------------------------------------
// Thread presence — transient SystemEvent variants project to the
// thread_presence table without writing to the events table.
// -----------------------------------------------------------------------

#[tokio::test]
async fn thread_focused_is_transient_and_not_persisted() {
    let event = SystemEvent::ThreadFocused {
        thread_id: Uuid::new_v4(),
        device_id: "dev-1".into(),
    };
    assert!(!event.is_persisted(), "ThreadFocused must be transient");
    assert_eq!(event.event_type(), "ThreadFocused");
    assert_eq!(event.aggregate(), "presence");

    let event = SystemEvent::ThreadUnfocused {
        thread_id: Uuid::new_v4(),
        device_id: "dev-1".into(),
    };
    assert!(!event.is_persisted(), "ThreadUnfocused must be transient");
    assert_eq!(event.event_type(), "ThreadUnfocused");
    assert_eq!(event.aggregate(), "presence");
}

#[tokio::test]
async fn thread_focused_sse_json_uses_typed_envelope() {
    let thread = Uuid::new_v4();
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::ThreadFocused {
            thread_id: thread,
            device_id: "dev-1".into(),
        }),
    };
    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "ThreadFocused");
    assert_eq!(json["data"]["thread_id"], thread.to_string());
    assert_eq!(json["data"]["device_id"], "dev-1");
}

#[tokio::test]
async fn emit_thread_focused_writes_to_thread_presence_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    bus.emit(BusEvent::System(SystemEvent::ThreadFocused {
        thread_id: thread,
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();

    let devices = crate::core::ThreadPresenceStore::devices_focused_on(&pool, thread)
        .await
        .unwrap();
    assert_eq!(devices, vec!["dev-1".to_string()]);

    // The events table should NOT have a row for this — transient.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type IN ('ThreadFocused', 'ThreadUnfocused')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "ThreadFocused must not be persisted to events table"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_focused_heartbeat_does_not_broadcast() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();
    let thread = Uuid::new_v4();

    // First focus — should broadcast.
    bus.emit(BusEvent::System(SystemEvent::ThreadFocused {
        thread_id: thread,
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();
    let received = rx.try_recv();
    assert!(received.is_ok(), "first ThreadFocused must broadcast");

    // Heartbeat — same device, same thread. Should NOT broadcast.
    bus.emit(BusEvent::System(SystemEvent::ThreadFocused {
        thread_id: thread,
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();
    let received = rx.try_recv();
    assert!(
        received.is_err(),
        "heartbeat ThreadFocused must NOT broadcast (would wake every SSE subscriber every 30s)"
    );

    // The DB row is still there — just no broadcast.
    let devices = crate::core::ThreadPresenceStore::devices_focused_on(&pool, thread)
        .await
        .unwrap();
    assert_eq!(devices, vec!["dev-1".to_string()]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_unfocused_for_unfocused_device_does_not_broadcast() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();
    let thread = Uuid::new_v4();

    // Unfocus a device that was never focused — should NOT broadcast.
    bus.emit(BusEvent::System(SystemEvent::ThreadUnfocused {
        thread_id: thread,
        device_id: "dev-never".into(),
    }))
    .await
    .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "ThreadUnfocused with no matching row must not broadcast"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn emit_thread_unfocused_removes_projection_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    bus.emit(BusEvent::System(SystemEvent::ThreadFocused {
        thread_id: thread,
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();
    bus.emit(BusEvent::System(SystemEvent::ThreadUnfocused {
        thread_id: thread,
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();

    let devices = crate::core::ThreadPresenceStore::devices_focused_on(&pool, thread)
        .await
        .unwrap();
    assert!(
        devices.is_empty(),
        "ThreadUnfocused should remove the projection row"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
