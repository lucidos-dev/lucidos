use super::*;
use crate::engine::thread_events::ActorMode;

fn device_actor() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "dev-1".to_string(),
        label: "My Mac".to_string(),
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
    assert_eq!(actor["label"], "My Mac");
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
    // Smoke check that non-device actor variants survive the merge.
    let evt = SystemEvent::DomainEvent {
        event_type: "Test".into(),
        payload: serde_json::json!({"x": 1}),
        depth: 0,
        transient: false,
        actor: Some(MessageOrigin::Api {
            user_agent: Some("curl/8".to_string()),
            mode: ActorMode::Human,
            source_thread_id: None,
        }),
    };
    let payload = evt.to_payload();
    let actor = payload.get("actor").expect("actor present");
    assert_eq!(actor["kind"], "api");
    assert_eq!(actor["user_agent"], "curl/8");
}

#[test]
fn presence_check_serializes_with_type_tag() {
    // Wire shape consumed by the SDK SSE handler — see
    // crates/lucidos-app/src/store/actions/presence-pong.ts. PresenceCheck is
    // now a pure pong trigger: it carries no toast content (that moved to
    // NotificationToastRequested, emitted only on the push-suppressed branch
    // so a toast and an OS push can't both fire — see notifications.md §3-§4).
    let e = SystemEvent::PresenceCheck {
        notification_id: Uuid::nil(),
        event_id: None,
        deadline_ms: 250,
        sent_at_ms: 1_700_000_000_000,
        actor: None,
    };
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "PresenceCheck");
    assert_eq!(json["data"]["deadline_ms"], 250);
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
    // No toast content rides on PresenceCheck anymore.
    assert!(json["data"].get("title").is_none());
    assert!(json["data"].get("body").is_none());
    assert!(json["data"].get("thread_id").is_none());
    assert!(json["data"].get("app_id").is_none());
    assert!(json["data"].get("tap").is_none());
    // Optional fields stay omitted when None so the wire shape mirrors
    // the spec snippet exactly.
    assert!(json["data"].get("event_id").is_none());
    assert!(json["data"].get("actor").is_none());
}

#[test]
fn presence_check_is_transient_not_persisted() {
    // Spec §3 — PresenceCheck is broadcast, never written to events.
    let e = SystemEvent::PresenceCheck {
        notification_id: Uuid::nil(),
        event_id: None,
        deadline_ms: 250,
        sent_at_ms: 0,
        actor: None,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "presence");
    assert_eq!(e.event_type(), "PresenceCheck");
}

#[test]
fn notification_toast_requested_serializes_with_type_tag() {
    // §4 in-app toast trigger — emitted after the engine decides to suppress
    // the OS push. Wire shape consumed by the SDK handler (in-app-
    // notification-toast.ts). Carries the toast content so the page renders
    // without a re-fetch; `sent_at_ms` drives the page-side freshness gate.
    let nid = Uuid::nil();
    let tid = Uuid::nil();
    let e = SystemEvent::NotificationToastRequested {
        notification_id: nid,
        title: "Claude is asking".to_string(),
        body: "Pick one".to_string(),
        thread_id: Some(tid),
        event_id: None,
        app_id: None,
        tap: Tap::Modal,
        sent_at_ms: 1_700_000_000_000,
    };
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "NotificationToastRequested");
    assert_eq!(json["data"]["notification_id"], nid.to_string());
    assert_eq!(json["data"]["title"], "Claude is asking");
    assert_eq!(json["data"]["body"], "Pick one");
    assert_eq!(json["data"]["thread_id"], tid.to_string());
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
    // Tap::Modal is the serde default — present and shaped {"kind":"modal"}.
    assert_eq!(json["data"]["tap"], serde_json::json!({"kind": "modal"}));
    // None optionals stay omitted.
    assert!(json["data"].get("event_id").is_none());
    assert!(json["data"].get("app_id").is_none());
}

#[test]
fn notification_toast_requested_is_transient_on_notification_aggregate() {
    // Transient UI signal — broadcast to SSE, never written to events. Lives
    // on the `notification` aggregate alongside NotificationCreated/Read.
    let e = SystemEvent::NotificationToastRequested {
        notification_id: Uuid::nil(),
        title: String::new(),
        body: String::new(),
        thread_id: None,
        event_id: None,
        app_id: None,
        tap: Tap::Modal,
        sent_at_ms: 0,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "notification");
    assert_eq!(e.event_type(), "NotificationToastRequested");
    assert_eq!(e.aggregate_id(), Uuid::nil().to_string());
}

#[test]
fn native_push_requested_serializes_with_type_tag() {
    // §4 native desktop surface trigger — emitted on the push-ALLOWED branch
    // so a connected Tauri app shows a native macOS banner. Same wire shape as
    // NotificationToastRequested (the SDK reuses the decode), consumed by the
    // native-push handler. `sent_at_ms` drives the page-side freshness gate.
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
}

#[test]
fn native_push_requested_is_transient_on_notification_aggregate() {
    // Transient UI signal — broadcast to SSE, never written to events. Lives on
    // the `notification` aggregate alongside NotificationToastRequested (its
    // mutually-exclusive complement: toast on push-suppressed, native on
    // push-allowed).
    let e = SystemEvent::NativePushRequested {
        notification_id: Uuid::nil(),
        title: String::new(),
        body: String::new(),
        thread_id: None,
        event_id: None,
        app_id: None,
        tap: Tap::Modal,
        sent_at_ms: 0,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "notification");
    assert_eq!(e.event_type(), "NativePushRequested");
    assert_eq!(e.aggregate_id(), Uuid::nil().to_string());
}

#[test]
fn frontend_update_deferred_is_transient_on_engine_aggregate() {
    // Transient UI hint — broadcast to SSE, never written to events. Emitted
    // when a frontend-only Apply's served-client advance is deferred because an
    // engine version change is pending (INV-A); the page renders a keyed toast.
    // Lives on the `engine` aggregate alongside EngineSupervisorRespawned.
    let e = SystemEvent::FrontendUpdateDeferred {
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "FrontendUpdateDeferred");
    assert_eq!(e.aggregate_id(), "global");
    // Standard SystemEvent wire shape: `{type, data:{sent_at_ms}}`.
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "FrontendUpdateDeferred");
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
}

#[test]
fn frontend_update_stranded_is_transient_on_engine_aggregate() {
    // Sibling of FrontendUpdateDeferred, and deliberately NOT the same event: a
    // stranded Apply is never arriving, so it must not reuse a message that
    // promises "on Switch". Transient, `engine` aggregate, same wire shape.
    let e = SystemEvent::FrontendUpdateStranded {
        served_dir: "/w/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist".into(),
        served_in_worktree: true,
        build_error: None,
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "FrontendUpdateStranded");
    assert_eq!(e.aggregate_id(), "global");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "FrontendUpdateStranded");
    // The page needs BOTH the path and the cause flag to give correct advice.
    assert_eq!(
        json["data"]["served_dir"],
        "/w/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist"
    );
    assert_eq!(json["data"]["served_in_worktree"], true);
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
    // Omitted rather than null when the build-watch said nothing, so a client
    // that never heard of the field sees exactly the body it always did.
    assert!(json["data"].get("build_error").is_none());
}

#[test]
fn a_failing_build_rides_along_with_the_stranded_signal() {
    // The other way an Apply strands. Until this field existed the message
    // could only ask "is the build-watch running?", while the real answer sat
    // in a log file nobody reads.
    let e = SystemEvent::FrontendUpdateStranded {
        served_dir: "/w/dev/crates/lucidos-app/dist".into(),
        served_in_worktree: false,
        build_error: Some("Rollup failed to resolve import \"jsqr\"".into()),
        sent_at_ms: 1_700_000_000_000,
    };
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(
        json["data"]["build_error"],
        "Rollup failed to resolve import \"jsqr\""
    );
}

#[test]
fn served_frontend_advanced_is_transient_on_engine_aggregate() {
    // Transient UI signal — broadcast to SSE, never written to events. Emitted
    // when a dev engine advances its served-frontend snapshot to the shared dist/
    // after a PEER workspace's frontend-only Apply (INV-A-gated). The connected
    // client re-runs syncClientUpdateFromBuild. Lives on the `engine` aggregate
    // alongside FrontendUpdateDeferred.
    let e = SystemEvent::ServedFrontendAdvanced {
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "ServedFrontendAdvanced");
    assert_eq!(e.aggregate_id(), "global");
    // Standard SystemEvent wire shape: `{type, data:{sent_at_ms}}`.
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "ServedFrontendAdvanced");
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
}

#[test]
fn frontend_preview_events_are_transient_and_keyed_by_thread() {
    // Transient UI signals for the supervised Vite dev server showing a
    // coding-agent worktree (engine::frontend_preview). On the `engine`
    // aggregate alongside ServedFrontendAdvanced, but UNLIKE its neighbours they
    // are keyed by thread: one preview per thread, one slot at a time.
    let thread_id = uuid::Uuid::parse_str("2951200f-0652-4ee2-baa3-433d608983d8").unwrap();
    let started = SystemEvent::FrontendPreviewStarted {
        thread_id,
        port: 6173,
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!started.is_persisted());
    assert_eq!(started.aggregate(), "engine");
    assert_eq!(started.event_type(), "FrontendPreviewStarted");
    assert_eq!(started.aggregate_id(), thread_id.to_string());
    let json = serde_json::to_value(&started).unwrap();
    assert_eq!(json["type"], "FrontendPreviewStarted");
    assert_eq!(json["data"]["port"], 6173);
    assert_eq!(json["data"]["thread_id"], thread_id.to_string());
    // The port travels, never a URL: only the page knows whether the user is on
    // localhost or a Tailscale name, so it composes the href itself.
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
    // Transient UI POKE — broadcast to SSE, never written to events. Emitted on
    // every dev background-rebuild state transition so the connected client learns
    // of a build over the live stream instead of the throttled version-status
    // poll. The page handler re-runs the authoritative checkEngineVersion GET.
    // Lives on the `engine` aggregate alongside ServedFrontendAdvanced.
    let e = SystemEvent::EngineBuildStateChanged {
        state: "building".to_string(),
        sent_at_ms: 1_700_000_000_000,
    };
    assert!(!e.is_persisted());
    assert_eq!(e.aggregate(), "engine");
    assert_eq!(e.event_type(), "EngineBuildStateChanged");
    assert_eq!(e.aggregate_id(), "global");
    // Standard SystemEvent wire shape: `{type, data:{state, sent_at_ms}}`.
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "EngineBuildStateChanged");
    assert_eq!(json["data"]["state"], "building");
    assert_eq!(json["data"]["sent_at_ms"], 1_700_000_000_000_i64);
}

#[test]
fn native_push_dismiss_requested_is_transient_on_notification_aggregate() {
    // Transient cross-device dismiss signal — broadcast to SSE, never written to
    // events. `Some(id)` removes one delivered native banner on connected Tauri
    // apps; `None` removes all. Lives on the `notification` aggregate.
    let one = SystemEvent::NativePushDismissRequested {
        notification_id: Some(Uuid::nil()),
        sent_at_ms: 0,
    };
    assert!(!one.is_persisted());
    assert_eq!(one.aggregate(), "notification");
    assert_eq!(one.event_type(), "NativePushDismissRequested");
    assert_eq!(one.aggregate_id(), Uuid::nil().to_string());

    let all = SystemEvent::NativePushDismissRequested {
        notification_id: None,
        sent_at_ms: 0,
    };
    assert!(!all.is_persisted());
    // `None` (dismiss-all) keys on the sentinel "all" rather than a single id.
    assert_eq!(all.aggregate_id(), "all");

    // Wire shape: `None` omits the field; `Some` carries it.
    let json_all = serde_json::to_value(&all).unwrap();
    assert_eq!(json_all["type"], "NativePushDismissRequested");
    assert!(json_all["data"].get("notification_id").is_none());
    let json_one = serde_json::to_value(&one).unwrap();
    assert_eq!(json_one["data"]["notification_id"], Uuid::nil().to_string());
}

/// `PERSISTED_TYPE_NAMES` is the one list behind both `is_persisted` and
/// `is_persisted_type_name`, so a typo in it silently stops an event being
/// written. Every entry must be a real wire name, which `RESERVED_TYPE_NAMES`
/// already enumerates and `reserved_type_names_match_event_type` already pins.
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

/// `DomainEvent` is a transport variant: no row is ever stored under that
/// literal name, so nothing may wait on it. Its persistence is decided by the
/// `transient` field instead, which is the one field-dependent answer.
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

/// The subscribe side reads the same answer as the write side. A backup
/// completion is a durable fact, its progress tick is a UI frame.
#[test]
fn the_name_keyed_predicate_agrees_with_the_event_keyed_one() {
    assert!(SystemEvent::is_persisted_type_name("BackupCompleted"));
    assert!(SystemEvent::is_persisted_type_name("BackupFailed"));
    assert!(!SystemEvent::is_persisted_type_name("BackupProgress"));
    assert!(!SystemEvent::is_persisted_type_name("Toast"));
    assert!(!SystemEvent::is_persisted_type_name("NotAnEventAtAll"));
}
