use super::*;

#[test]
fn message_origin_engine_serializes_with_kind_engine() {
    let origin = MessageOrigin::Engine {
        reason: EngineReason::ContinuationStarted,
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json["kind"], "engine");
    assert_eq!(json["reason"]["kind"], "continuation_started");
}

/// `EngineReason` legacy serde alias: old DB rows persisted with
/// `{"kind":"session_recovered"}` (and the even older
/// `{"kind":"session_resumed"}` if any survive) must still deserialize as the
/// renamed `ContinuationStarted` variant. Without the alias the projection
/// crashes on any historical row using the old name.
#[test]
fn engine_reason_continuation_started_accepts_legacy_session_recovered_alias() {
    let v: EngineReason = serde_json::from_str(r#"{"kind":"session_recovered"}"#).unwrap();
    assert_eq!(v, EngineReason::ContinuationStarted);
}

/// `MessageOrigin::System` serializes as `{"kind":"system"}` with NO
/// other fields. The frontend's MessageOrigin union has `{ kind: 'system' }`
/// (no reason / no metadata) — adding fields here would break that contract.
/// Distinct from Engine: System means the host killed the process; Engine
/// means the engine deliberately took an action.
#[test]
fn message_origin_system_serializes_with_kind_system_no_other_fields() {
    let origin = MessageOrigin::System;
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json, serde_json::json!({"kind": "system"}));
}

/// System is intrinsically engine-mode (deterministic, non-human, non-agent).
/// Mirrors `MessageOrigin::Engine`'s mode — the chip differentiates via
/// label override (System vs Lucidos Engine), not via mode.
#[test]
fn message_origin_system_mode_is_engine() {
    assert_eq!(MessageOrigin::System.mode(), ActorMode::Engine);
}

/// `MessageOrigin::system()` is the canonical constructor — emit sites use
/// it for the "host killed the process" attribution (orphan recovery,
/// shutdown, safety net, post-restart abort marker).
#[test]
fn message_origin_system_constructor() {
    assert!(matches!(MessageOrigin::system(), MessageOrigin::System));
}

#[test]
fn message_origin_engine_scheduler_carries_trigger_metadata() {
    let trigger_id = uuid::Uuid::new_v4().to_string();
    let origin = MessageOrigin::Engine {
        reason: EngineReason::Scheduler {
            trigger_id: trigger_id.clone(),
            trigger_name: Some("nightly-backup".to_string()),
        },
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json["reason"]["kind"], "scheduler");
    assert_eq!(json["reason"]["trigger_id"], trigger_id);
    assert_eq!(json["reason"]["trigger_name"], "nightly-backup");
}

#[test]
fn message_origin_engine_plugin_auto_update_carries_marketplace_metadata() {
    let origin = MessageOrigin::Engine {
        reason: EngineReason::PluginAutoUpdate {
            plugin_id: "browser-learning".to_string(),
            marketplace_id: "core".to_string(),
            marketplace_name: "Core".to_string(),
        },
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json["reason"]["kind"], "plugin_auto_update");
    assert_eq!(json["reason"]["plugin_id"], "browser-learning");
    assert_eq!(json["reason"]["marketplace_id"], "core");
    assert_eq!(json["reason"]["marketplace_name"], "Core");
}

#[test]
fn message_origin_engine_round_trips_through_serde() {
    let original = MessageOrigin::Engine {
        reason: EngineReason::HardenRetrigger,
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn actor_mode_serializes_lowercase_strings() {
    assert_eq!(
        serde_json::to_string(&ActorMode::Human).unwrap(),
        "\"human\""
    );
    assert_eq!(
        serde_json::to_string(&ActorMode::Agent).unwrap(),
        "\"agent\""
    );
    assert_eq!(
        serde_json::to_string(&ActorMode::Engine).unwrap(),
        "\"engine\""
    );
}

#[test]
fn actor_mode_deserializes_lowercase_strings() {
    assert_eq!(
        serde_json::from_str::<ActorMode>("\"human\"").unwrap(),
        ActorMode::Human
    );
    assert_eq!(
        serde_json::from_str::<ActorMode>("\"agent\"").unwrap(),
        ActorMode::Agent
    );
    assert_eq!(
        serde_json::from_str::<ActorMode>("\"engine\"").unwrap(),
        ActorMode::Engine
    );
}

#[test]
fn message_origin_thread_link_defaults_mode_to_agent_when_missing() {
    let json = r#"{
            "kind": "thread_link",
            "thread_id": "00000000-0000-0000-0000-000000000001"
        }"#;
    let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
    match parsed {
        MessageOrigin::ThreadLink {
            mode, direction, ..
        } => {
            assert_eq!(mode, ActorMode::Agent);
            assert_eq!(direction, ThreadDirection::Parent);
        }
        other => panic!("expected ThreadLink, got {:?}", other),
    }
}

/// Historical DB rows persisted under the old variant name. The
/// `serde(alias = "parent_thread")` + default `direction` keep them
/// readable as `ThreadLink { direction: Parent }`.
#[test]
fn message_origin_legacy_parent_thread_kind_deserializes_as_thread_link() {
    let json = r#"{
            "kind": "parent_thread",
            "thread_id": "00000000-0000-0000-0000-000000000001",
            "mode": "engine"
        }"#;
    let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
    match parsed {
        MessageOrigin::ThreadLink {
            mode, direction, ..
        } => {
            assert_eq!(mode, ActorMode::Engine);
            assert_eq!(direction, ThreadDirection::Parent);
        }
        other => panic!(
            "expected ThreadLink (from parent_thread alias), got {:?}",
            other
        ),
    }
}

#[test]
fn message_origin_workspace_defaults_mode_to_human_when_missing() {
    let json = r#"{ "kind": "workspace", "workspace": "personal" }"#;
    let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
    match parsed {
        MessageOrigin::Workspace { mode, .. } => assert_eq!(mode, ActorMode::Human),
        other => panic!("expected Workspace, got {:?}", other),
    }
}

#[test]
fn message_origin_thread_link_round_trips_with_explicit_engine_mode() {
    let original = MessageOrigin::ThreadLink {
        thread_id: uuid::Uuid::new_v4(),
        title: Some("recovered".into()),
        spawning_event_id: None,
        mode: ActorMode::Engine,
        direction: ThreadDirection::Parent,
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn message_origin_thread_link_child_round_trips() {
    let original = MessageOrigin::ThreadLink {
        thread_id: uuid::Uuid::new_v4(),
        title: Some("child task".into()),
        spawning_event_id: Some(uuid::Uuid::new_v4()),
        mode: ActorMode::Agent,
        direction: ThreadDirection::Child,
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn message_origin_mode_derives_human_for_device_and_api() {
    let device = MessageOrigin::Device {
        device_id: "d".into(),
        label: "l".into(),
    };
    let api = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Human,
        source_thread_id: None,
    };
    assert_eq!(device.mode(), ActorMode::Human);
    assert_eq!(api.mode(), ActorMode::Human);
}

#[test]
fn message_origin_mode_derives_engine_for_engine_variant() {
    let origin = MessageOrigin::Engine {
        reason: EngineReason::ContinuationStarted,
    };
    assert_eq!(origin.mode(), ActorMode::Engine);
}

#[test]
fn message_origin_mode_reads_field_for_workspace_and_thread_link() {
    let ws = MessageOrigin::Workspace {
        workspace: "x".into(),
        thread_id: None,
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
    };
    let tl = MessageOrigin::ThreadLink {
        thread_id: uuid::Uuid::new_v4(),
        title: None,
        spawning_event_id: None,
        mode: ActorMode::Engine,
        direction: ThreadDirection::Parent,
    };
    assert_eq!(ws.mode(), ActorMode::Agent);
    assert_eq!(tl.mode(), ActorMode::Engine);
}
