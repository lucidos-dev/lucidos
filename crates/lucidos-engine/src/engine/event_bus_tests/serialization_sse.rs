use super::super::*;
use super::*;

#[test]
fn bus_event_variants_are_constructable() {
    let thread_event = BusEvent::Thread {
        thread_id: Uuid::new_v4(),
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "hello".into(),
            user_image_hashes: vec![],
            device_id: None,
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
        event: ThreadEvent::CumulativeTextUpdated {
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
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
            event: ThreadEvent::CumulativeTextUpdated { text: "hi".into() },
            meta: EventMeta::NONE,
        },
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
            event: ThreadEvent::CumulativeTextUpdated {
                text: "hello".into(),
            },
            meta: EventMeta::NONE,
        },
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
                provider: None,
                voice_session_id: None,
                text: "hello".into(),
                user_image_hashes: vec![],
                device_id: None,
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
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
                provider: None,
                voice_session_id: None,
                text: "fix bug".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..Default::default()
            },
        },
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
                provider: None,
                voice_session_id: None,
                text: "hello".into(),
                user_image_hashes: vec![],
                device_id: None,
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
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
            event: ThreadEvent::CumulativeTextUpdated {
                text: "chunk".into(),
            },
            meta: EventMeta::NONE,
        },
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "ThreadEvent");
    assert_eq!(json["data"]["thread_id"], tid.to_string());
    assert!(
        json["data"]["seq"].is_null(),
        "transient events should not have seq"
    );
    assert_eq!(json["data"]["event"]["type"], "CumulativeTextUpdated");
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
            thread_id: None,
            event_id: None,
            tap: crate::scheduler::notifications::Tap::Modal,
            actor: None,
        }),
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
            set_aside: vec![],
            applied: vec![],
            total_pending: 0,
            restart_required: false,
            apply_estimates: crate::engine::apply_estimate::ApplyEstimates {
                hardening: None,
                resolving_conflict: Some(crate::engine::apply_estimate::PhaseEstimate {
                    typical_secs: 1080,
                    runs: 32,
                }),
            },
        }),
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "ChangesUpdated");
    assert_eq!(json["data"]["total_pending"], 0);
    assert_eq!(json["data"]["restart_required"], false);
    assert_eq!(
        json["data"]["apply_estimates"],
        serde_json::json!({
            "hardening": null,
            "resolving_conflict": { "typical_secs": 1080, "runs": 32 },
        })
    );
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
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "MemoryRebuildProgress");
    assert_eq!(json["data"]["percent"], 50);
}

/// The `EmbeddingModelStatusChanged` payload and the
/// `GET /api/v1/memory/embedding-model-status` snapshot are ONE shape by
/// contract: a client that arrives mid-download (the normal case on a fresh
/// workspace) reads the snapshot, and must not have to translate it into what
/// the stream would have told it. Pin the two serializations against each other.
#[test]
fn embedding_model_status_event_matches_the_rest_snapshot() {
    let status = crate::memory::EmbeddingModelStatus {
        model_id: "multilingual-e5-small".into(),
        load_state: crate::memory::EmbeddingModelLoadState::Downloading {
            downloaded_bytes: 244_000_000,
            total_bytes: 488_000_000,
        },
    };
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::EmbeddingModelStatusChanged {
            model_id: status.model_id.clone(),
            load_state: status.load_state.clone(),
        }),
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "EmbeddingModelStatusChanged");
    assert_eq!(
        json["data"],
        serde_json::to_value(&status).unwrap(),
        "the SSE payload and the REST snapshot must serialize identically"
    );
}

/// The `kind` discriminator and its kebab-case values are what the frontend
/// switches on, so they are wire contract, not an implementation detail.
#[test]
fn embedding_model_load_state_wire_tags_are_stable() {
    use crate::memory::EmbeddingModelLoadState as S;
    let cases = [
        (
            S::Downloading {
                downloaded_bytes: 7,
                total_bytes: 9,
            },
            "downloading",
        ),
        (S::Loading, "loading"),
        (S::Ready, "ready"),
        (S::Waiting { attempt: 3 }, "waiting"),
        (
            S::Failed {
                message: "boom".into(),
            },
            "failed",
        ),
    ];
    for (state, expected) in cases {
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["kind"], expected, "wire tag drifted for {state:?}");
    }

    // The payload fields each variant carries, spot-checked where the UI reads
    // them: the progress bar needs both byte counts on the same object as the
    // tag (no nesting), and a terminal failure has to carry its reason.
    let downloading = serde_json::to_value(S::Downloading {
        downloaded_bytes: 7,
        total_bytes: 9,
    })
    .unwrap();
    assert_eq!(downloading["downloaded_bytes"], 7);
    assert_eq!(downloading["total_bytes"], 9);
    let failed = serde_json::to_value(S::Failed {
        message: "boom".into(),
    })
    .unwrap();
    assert_eq!(failed["message"], "boom");
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
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "BackupProgress");
    assert_eq!(json["data"]["phase"], "uploading");
}

#[test]
fn system_backup_completed_matches_server_event_shape() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::BackupCompleted {
            filename: "lucidos-backup-myws-20260504-090000.enc".into(),
            size_bytes: 927_401_289,
            started_at: Utc::now(),
            finished_at: Utc::now(),
        }),
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "BackupCompleted");
    assert_eq!(
        json["data"]["filename"],
        "lucidos-backup-myws-20260504-090000.enc"
    );
    assert_eq!(json["data"]["size_bytes"], 927_401_289u64);

    let event = SystemEvent::BackupCompleted {
        filename: "f".into(),
        size_bytes: 1,
        started_at: Utc::now(),
        finished_at: Utc::now(),
    };
    assert!(
        event.is_persisted(),
        "BackupCompleted is persisted — it is the durable backup run history"
    );
    assert_eq!(event.aggregate(), "ops");
}

#[test]
fn system_backup_failed_matches_server_event_shape() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::BackupFailed {
            error: "Token refresh failed (invalid_grant)".into(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
        }),
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(json["type"], "BackupFailed");
    assert_eq!(
        json["data"]["error"],
        "Token refresh failed (invalid_grant)"
    );

    let event = SystemEvent::BackupFailed {
        error: "x".into(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
    };
    assert!(
        event.is_persisted(),
        "BackupFailed is persisted — it is part of the durable backup run history"
    );
    assert_eq!(event.aggregate(), "ops");
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
            actor: None,
        }),
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
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
            thread_id: None,
            event_id: None,
            tap: crate::scheduler::notifications::Tap::Modal,
            actor: None,
        },
        NotificationRead {
            id: "x".into(),
            actor: None,
        },
        NotificationsAllRead { actor: None },
        RecommendedCleanupStarted { actor: None },
        RecommendedCleanupProgress { done: 0, total: 0 },
        RecommendedCleanupCompleted {
            removed_count: 0,
            cleaned_count: 0,
            freed_bytes: 0,
        },
        RecommendedCleanupFailed { error: "e".into() },
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
        EmbeddingModelStatusChanged {
            model_id: "m".into(),
            load_state: crate::memory::EmbeddingModelLoadState::Loading,
        },
        ChangesUpdated {
            pending: vec![],
            set_aside: vec![],
            applied: vec![],
            total_pending: 0,
            restart_required: false,
            apply_estimates: Default::default(),
        },
        BackupProgress {
            phase: "p".into(),
            progress: 0,
            total: 0,
        },
        BackupCompleted {
            filename: "lucidos-backup-x-20260504-090000.enc".into(),
            size_bytes: 100,
            started_at: Utc::now(),
            finished_at: Utc::now(),
        },
        BackupFailed {
            error: "boom".into(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
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
            actor: None,
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
        EmailSent {
            account: "primary".into(),
            to: vec!["a@b.com".into()],
            cc: vec![],
            bcc: vec![],
            subject: "Hi".into(),
            attachment_count: 0,
            actor: None,
        },
        ProxyModulesReloaded {
            count: 0,
            names: vec![],
            actor: None,
        },
        PermissionGrantsChanged {
            grant_file: crate::core::GrantFile::CodingAgentTools,
            patterns: vec!["Bash(git:*)".into()],
            actor: None,
        },
        CredentialRevealed {
            service_name: "anthropic".into(),
            auth_type: "api_key".into(),
            actor: None,
        },
        BackupKeyRevealed {
            minted: false,
            actor: None,
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

/// The live frame of one thread event, as `to_sse_json` puts it on the wire.
fn live_event_json(event: ThreadEvent) -> serde_json::Value {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(7),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event,
            meta: EventMeta::NONE,
        },
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };
    let frame: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    frame["data"]["event"].clone()
}

#[test]
fn a_live_coding_agent_result_leaves_its_text_for_the_modal_to_fetch() {
    let event = live_event_json(ThreadEvent::CodingAgentToolResult {
        name: "Bash".into(),
        result: "test output\n".repeat(1_000),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        tool_use_id: "toolu_1".into(),
    });
    assert!(
        event.get("result").is_none(),
        "live frame carried the output: {event}"
    );
    assert_eq!(event["result_stripped"], true);
    assert_eq!(event["name"], "Bash");
    assert_eq!(event["tool_use_id"], "toolu_1");
    assert_eq!(event["type"], "CodingAgentToolResult");
}

#[test]
fn a_live_chat_tool_result_keeps_its_text() {
    let event = live_event_json(ThreadEvent::ToolResult {
        name: "read_file".into(),
        result: "file body".into(),
        images: vec![],
        success: true,
        tool_called_event_id: None,
    });
    assert_eq!(event["result"], "file body");
    assert!(event.get("result_stripped").is_none());
}

#[test]
fn a_live_coding_agent_call_leaves_its_args_for_the_modal_to_fetch() {
    let event = live_event_json(ThreadEvent::CodingAgentToolCalled {
        name: "Write".into(),
        args: serde_json::json!({
            "file_path": "/tmp/notes.ts",
            "content": "export const x = 1;\n".repeat(1_000),
        }),
        description: "Write notes.ts".into(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        tool_use_id: "toolu_1".into(),
    });
    assert!(
        event.get("args").is_none(),
        "live frame carried the args: {event}"
    );
    assert_eq!(event["args_stripped"], true);
    assert_eq!(event["description"], "Write notes.ts");
    assert_eq!(event["name"], "Write");
    assert_eq!(event["tool_use_id"], "toolu_1");
}

#[test]
fn a_live_chat_call_without_a_description_is_described_before_its_args_go() {
    let event = live_event_json(ThreadEvent::ToolCalled {
        name: "run_bash".into(),
        args: serde_json::json!({ "command": "ls -la" }),
        description: String::new(),
    });
    assert!(
        event.get("args").is_none(),
        "live frame carried the args: {event}"
    );
    assert_eq!(event["args_stripped"], true);
    let description = event["description"].as_str().unwrap_or_default();
    assert!(!description.is_empty(), "no label left: {event}");
}

#[test]
fn a_live_generate_image_call_keeps_the_prompt_its_image_is_described_by() {
    let event = live_event_json(ThreadEvent::ToolCalled {
        name: "generate_image".into(),
        args: serde_json::json!({ "prompt": "a red bicycle" }),
        description: "Generate image".into(),
    });
    assert_eq!(event["args"]["prompt"], "a red bicycle");
    assert!(event.get("args_stripped").is_none());
}
