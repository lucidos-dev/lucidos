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
