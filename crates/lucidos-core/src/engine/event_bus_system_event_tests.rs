use super::*;
use crate::engine::thread_events::ActorMode;

fn device_actor() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "dev-1".to_string(),
    }
}

#[test]
fn domain_event_to_payload_injects_actor_into_object() {
    let evt = SystemEvent::DomainEvent {
        event_type: "Test".into(),
        payload: serde_json::json!({"hello": "world"}),
        depth: 0,
        transient: false,
        actor: Some(device_actor()),
    };
    let payload = evt.to_payload();
    assert_eq!(payload["hello"], "world", "original keys preserved");
    let actor = payload.get("actor").expect("actor must be merged in");
    assert_eq!(actor["kind"], "device");
    assert_eq!(actor["device_id"], "dev-1");
    assert!(
        actor.get("label").is_none(),
        "a device actor stores its id, never its name"
    );
}

#[test]
fn domain_event_to_payload_skips_actor_when_none() {
    let evt = SystemEvent::DomainEvent {
        event_type: "Test".into(),
        payload: serde_json::json!({"hello": "world"}),
        depth: 0,
        transient: false,
        actor: None,
    };
    let payload = evt.to_payload();
    assert_eq!(payload, serde_json::json!({"hello": "world"}));
}

#[test]
fn domain_event_to_payload_with_api_actor_keeps_mode_and_user_agent() {
    let evt = SystemEvent::DomainEvent {
        event_type: "Test".into(),
        payload: serde_json::json!({"x": 1}),
        depth: 0,
        transient: false,
        actor: Some(MessageOrigin::Api {
            user_agent: Some("curl/8".to_string()),
            mode: ActorMode::Human,
            source_thread_id: None,
            emitting_trigger_id: None,
        }),
    };
    let payload = evt.to_payload();
    let actor = payload.get("actor").expect("actor present");
    assert_eq!(actor["kind"], "api");
    assert_eq!(actor["user_agent"], "curl/8");
}

#[test]
fn presence_check_serializes_with_type_tag() {
    let nid = Uuid::nil();
    let tid = Uuid::nil();
    let e = SystemEvent::PresenceCheck {
        notification_id: nid,
        title: "Claude is asking".to_string(),
        body: "Pick one".to_string(),
        thread_id: Some(tid),
        event_id: None,
        app_id: None,
        tap: Tap::Modal,
        deadline_ms: 250,
        sent_at_ms: 1_700_000_000_000,
        audience: Some(vec!["my-iphone".to_string()]),
        actor: None,
    };
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "PresenceCheck");
    assert_eq!(json["data"]["notification_id"], nid.to_string());
    assert_eq!(json["data"]["title"], "Claude is asking");
    assert_eq!(json["data"]["body"], "Pick one");
    assert_eq!(json["data"]["thread_id"], tid.to_string());
    assert_eq!(json["data"]["deadline_ms"], 250);
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
    assert_eq!(json["data"]["tap"], serde_json::json!({"kind": "modal"}));
    assert!(json["data"].get("event_id").is_none());
    assert!(json["data"].get("app_id").is_none());
    assert!(json["data"].get("actor").is_none());
    assert_eq!(json["data"]["audience"], serde_json::json!(["my-iphone"]));
}

#[test]
fn presence_check_is_transient_not_persisted() {
    let e = SystemEvent::PresenceCheck {
        notification_id: Uuid::nil(),
        title: String::new(),
        body: String::new(),
        thread_id: None,
        event_id: None,
        app_id: None,
        tap: Tap::Modal,
        deadline_ms: 250,
        sent_at_ms: 0,
        audience: None,
        actor: None,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "presence");
    assert_eq!(e.event_type(), "PresenceCheck");
}

#[test]
fn native_push_requested_serializes_with_type_tag() {
    let nid = Uuid::nil();
    let tid = Uuid::nil();
    let e = SystemEvent::NativePushRequested {
        notification_id: nid,
        title: "Claude is asking".to_string(),
        body: "Pick one".to_string(),
        thread_id: Some(tid),
        event_id: None,
        app_id: None,
        tap: Tap::Modal,
        sent_at_ms: 1_700_000_000_000,
        audience: None,
    };
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "NativePushRequested");
    assert_eq!(json["data"]["notification_id"], nid.to_string());
    assert_eq!(json["data"]["title"], "Claude is asking");
    assert_eq!(json["data"]["body"], "Pick one");
    assert_eq!(json["data"]["thread_id"], tid.to_string());
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
    assert_eq!(json["data"]["tap"], serde_json::json!({"kind": "modal"}));
    assert!(json["data"].get("event_id").is_none());
    assert!(json["data"].get("app_id").is_none());
    assert!(json["data"].get("audience").is_none());
}

#[test]
fn native_push_requested_is_transient_on_notification_aggregate() {
    let e = SystemEvent::NativePushRequested {
        notification_id: Uuid::nil(),
        title: String::new(),
        body: String::new(),
        thread_id: None,
        event_id: None,
        app_id: None,
        tap: Tap::Modal,
        sent_at_ms: 0,
        audience: None,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "notification");
    assert_eq!(e.event_type(), "NativePushRequested");
    assert_eq!(e.aggregate_id(), Uuid::nil().to_string());
}

#[test]
fn frontend_update_deferred_is_transient_on_engine_aggregate() {
    let e = SystemEvent::FrontendUpdateDeferred {
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "FrontendUpdateDeferred");
    assert_eq!(e.aggregate_id(), "global");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "FrontendUpdateDeferred");
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
}

#[test]
fn frontend_update_stranded_is_transient_on_engine_aggregate() {
    let e = SystemEvent::FrontendUpdateStranded {
        served_dir: "/w/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist".into(),
        served_in_worktree: true,
        build_error: None,
        build_watch_stopped: false,
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "FrontendUpdateStranded");
    assert_eq!(e.aggregate_id(), "global");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "FrontendUpdateStranded");
    assert_eq!(
        json["data"]["served_dir"],
        "/w/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist"
    );
    assert_eq!(json["data"]["served_in_worktree"], true);
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
    assert!(json["data"].get("build_error").is_none());
}

#[test]
fn a_failing_build_rides_along_with_the_stranded_signal() {
    let e = SystemEvent::FrontendUpdateStranded {
        served_dir: "/w/dev/crates/lucidos-app/dist".into(),
        served_in_worktree: false,
        build_error: Some("Rollup failed to resolve import \"jsqr\"".into()),
        build_watch_stopped: false,
        sent_at_ms: 1_700_000_000_000,
    };
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(
        json["data"]["build_error"],
        "Rollup failed to resolve import \"jsqr\""
    );
}

#[test]
fn a_stopped_build_watch_rides_along_with_the_stranded_signal() {
    let e = SystemEvent::FrontendUpdateStranded {
        served_dir: "/w/dev/crates/lucidos-app/dist".into(),
        served_in_worktree: false,
        build_error: None,
        build_watch_stopped: true,
        sent_at_ms: 1_700_000_000_000,
    };
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["data"]["build_watch_stopped"], true);

    let running = SystemEvent::FrontendUpdateStranded {
        served_dir: "/w/dev/crates/lucidos-app/dist".into(),
        served_in_worktree: false,
        build_error: None,
        build_watch_stopped: false,
        sent_at_ms: 1_700_000_000_000,
    };
    let json = serde_json::to_value(&running).unwrap();
    assert_eq!(json["data"]["build_watch_stopped"], false);
}

#[test]
fn served_frontend_advanced_is_transient_on_engine_aggregate() {
    let e = SystemEvent::ServedFrontendAdvanced {
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "ServedFrontendAdvanced");
    assert_eq!(e.aggregate_id(), "global");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "ServedFrontendAdvanced");
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
}

#[test]
fn frontend_preview_events_are_transient_and_keyed_by_thread() {
    let thread_id = uuid::Uuid::parse_str("2951200f-0652-4ee2-baa3-433d608983d8").unwrap();
    let started = SystemEvent::FrontendPreviewStarted {
        thread_id,
        port: 6173,
        loopback_only: true,
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!started.is_persisted());
    assert_eq!(started.aggregate(), "engine");
    assert_eq!(started.event_type(), "FrontendPreviewStarted");
    assert_eq!(started.aggregate_id(), thread_id.to_string());
    let json = serde_json::to_value(&started).unwrap();
    assert_eq!(json["type"], "FrontendPreviewStarted");
    assert_eq!(json["data"]["port"], 6173);
    assert_eq!(json["data"]["loopback_only"], true);
    assert_eq!(json["data"]["thread_id"], thread_id.to_string());
    assert!(
        json["data"].get("url").is_none(),
        "the event must not carry a URL the engine cannot know"
    );

    let stopped = SystemEvent::FrontendPreviewStopped {
        thread_id,
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!stopped.is_persisted());
    assert_eq!(stopped.aggregate(), "engine");
    assert_eq!(stopped.event_type(), "FrontendPreviewStopped");
    assert_eq!(stopped.aggregate_id(), thread_id.to_string());
    assert_eq!(
        serde_json::to_value(&stopped).unwrap()["type"],
        "FrontendPreviewStopped"
    );
}

#[test]
fn engine_build_state_changed_is_transient_on_engine_aggregate() {
    let e = SystemEvent::EngineBuildStateChanged {
        state: "building".to_string(),
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "EngineBuildStateChanged");
    assert_eq!(e.aggregate_id(), "global");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "EngineBuildStateChanged");
    assert_eq!(json["data"]["state"], "building");
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
}

#[test]
fn frontend_refresh_state_changed_is_transient_on_engine_aggregate() {
    let e = SystemEvent::FrontendRefreshStateChanged {
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "FrontendRefreshStateChanged");
    assert_eq!(e.aggregate_id(), "global");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "FrontendRefreshStateChanged");
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
}

#[test]
fn frontend_build_watch_state_changed_is_transient_on_engine_aggregate() {
    let event = SystemEvent::FrontendBuildWatchStateChanged {
        sent_at_ms: 123_000,
    };
    assert!(!event.is_persisted());
    assert_eq!(event.aggregate(), "engine");
    assert_eq!(event.event_type(), "FrontendBuildWatchStateChanged");
    assert_eq!(event.aggregate_id(), "global");
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        serde_json::json!({"type": "FrontendBuildWatchStateChanged", "data": {"sent_at_ms": 123_000}})
    );
}

#[test]
fn native_push_dismiss_requested_is_transient_on_notification_aggregate() {
    let one = SystemEvent::NativePushDismissRequested {
        notification_id: Some(Uuid::nil()),
        sent_at_ms: 0,
        audience: Some(vec!["sam-phone".into()]),
    };
    assert!(!one.is_persisted());
    assert_eq!(one.aggregate(), "notification");
    assert_eq!(one.event_type(), "NativePushDismissRequested");
    assert_eq!(one.aggregate_id(), Uuid::nil().to_string());

    let all = SystemEvent::NativePushDismissRequested {
        notification_id: None,
        sent_at_ms: 0,
        audience: None,
    };
    assert!(!all.is_persisted());
    assert_eq!(all.aggregate_id(), "all");

    let json_all = serde_json::to_value(&all).unwrap();
    assert_eq!(json_all["type"], "NativePushDismissRequested");
    assert!(json_all["data"].get("notification_id").is_none());
    assert!(json_all["data"].get("audience").is_none());
    let json_one = serde_json::to_value(&one).unwrap();
    assert_eq!(json_one["data"]["notification_id"], Uuid::nil().to_string());
    assert_eq!(
        json_one["data"]["audience"],
        serde_json::json!(["sam-phone"])
    );
}

#[test]
fn every_persisted_name_is_a_real_reserved_name() {
    for name in SystemEvent::PERSISTED_TYPE_NAMES {
        assert!(
            SystemEvent::is_reserved_type_name(name),
            "'{name}' is in PERSISTED_TYPE_NAMES but is not a SystemEvent wire name"
        );
    }
}

#[test]
fn persisted_type_names_holds_no_duplicates() {
    let mut seen = std::collections::HashSet::new();
    for name in SystemEvent::PERSISTED_TYPE_NAMES {
        assert!(seen.insert(*name), "'{name}' appears twice");
    }
}

#[test]
fn domain_event_is_not_a_persisted_name_but_a_persisted_event() {
    assert!(!SystemEvent::is_persisted_type_name("DomainEvent"));

    let durable = SystemEvent::DomainEvent {
        event_type: "SleepImported".to_string(),
        payload: serde_json::json!({}),
        depth: 0,
        transient: false,
        actor: None,
    };
    assert!(durable.is_persisted());

    let ephemeral = SystemEvent::DomainEvent {
        event_type: "SleepImported".to_string(),
        payload: serde_json::json!({}),
        depth: 0,
        transient: true,
        actor: None,
    };
    assert!(!ephemeral.is_persisted());
}

#[test]
fn the_name_keyed_predicate_agrees_with_the_event_keyed_one() {
    assert!(SystemEvent::is_persisted_type_name("BackupCompleted"));
    assert!(SystemEvent::is_persisted_type_name("BackupFailed"));
    assert!(!SystemEvent::is_persisted_type_name("BackupProgress"));
    assert!(!SystemEvent::is_persisted_type_name("Toast"));
    assert!(!SystemEvent::is_persisted_type_name("NotAnEventAtAll"));
}

#[test]
fn a_default_shaped_row_gives_back_its_event() {
    let written = SystemEvent::ArtifactCreated {
        artifact_path: "notes.md".into(),
        commit: "abc123".into(),
        source: Some("chat".into()),
        writer_thread_id: Some(uuid::Uuid::new_v4()),
        writer: None,
    };
    let read = SystemEvent::from_stored(written.to_payload()).expect("round trip");
    assert_eq!(
        serde_json::to_value(&read).unwrap(),
        serde_json::to_value(&written).unwrap()
    );
}

#[test]
fn a_bespoke_row_is_refused_rather_than_misread() {
    let trigger = SystemEvent::TriggerExecuted {
        trigger_id: "t-1".into(),
        payload: serde_json::json!({"name": "nightly"}),
    };
    assert!(SystemEvent::from_stored(trigger.to_payload()).is_err());
}

#[test]
fn snapshot_keep_changes_replay_with_actor_and_retention_state() {
    let handle = crate::core::snapshot::handle::Handle::generate();
    for keep in [true, false] {
        let written = SystemEvent::SnapshotKeepChanged {
            workspace_id: "demo".into(),
            handle: handle.clone(),
            keep,
            actor: Some(device_actor()),
        };
        assert!(written.is_persisted());
        assert!(SystemEvent::is_persisted_type_name("SnapshotKeepChanged"));
        assert_eq!(written.aggregate(), "snapshot");
        assert_eq!(written.aggregate_id(), handle.as_str());
        let read = SystemEvent::from_stored(written.to_payload()).expect("keep change replay");
        assert_eq!(
            serde_json::to_value(read).unwrap(),
            serde_json::to_value(written).unwrap()
        );
    }
}

#[test]
fn engine_version_status_changed_is_transient_on_engine_aggregate() {
    let event = SystemEvent::EngineVersionStatusChanged { sent_at_ms: 42 };
    assert!(!event.is_persisted());
    assert_eq!(event.aggregate(), "engine");
    assert_eq!(event.event_type(), "EngineVersionStatusChanged");
    let json = serde_json::to_value(event).unwrap();
    assert_eq!(json["data"]["sent_at_ms"], 42);
}
