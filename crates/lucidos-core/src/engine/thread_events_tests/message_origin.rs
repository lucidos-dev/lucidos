use super::*;

#[test]
fn saved_draft_content_is_human_without_device_or_agent_authorship() {
    let origin = MessageOrigin::SavedDraft {
        submission_event_id: uuid::Uuid::new_v4(),
        submitted_by_thread_id: uuid::Uuid::new_v4(),
    };
    assert_eq!(origin.mode(), ActorMode::Human);
    assert_eq!(origin.source_thread_id(), None);
    assert_eq!(origin.agent(), None);
    let value = serde_json::to_value(&origin).unwrap();
    assert_eq!(value["kind"], "saved_draft");
    assert!(value.get("device_id").is_none());
    assert_eq!(
        serde_json::from_value::<MessageOrigin>(value).unwrap(),
        origin
    );
}

#[test]
fn message_origin_engine_serializes_with_kind_engine() {
    let origin = MessageOrigin::Engine {
        reason: EngineReason::ContinuationStarted,
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json["kind"], "engine");
    assert_eq!(json["reason"]["kind"], "continuation_started");
}

#[test]
fn engine_reason_continuation_started_accepts_legacy_session_recovered_alias() {
    let v: EngineReason = serde_json::from_str(r#"{"kind":"session_recovered"}"#).unwrap();
    assert_eq!(v, EngineReason::ContinuationStarted);
}

#[test]
fn message_origin_system_serializes_with_kind_system_no_other_fields() {
    let origin = MessageOrigin::System;
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json, serde_json::json!({"kind": "system"}));
}

#[test]
fn message_origin_system_mode_is_engine() {
    assert_eq!(MessageOrigin::System.mode(), ActorMode::Engine);
}

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
            actor_email: None,
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
    let json = r#"{ "kind": "workspace", "workspace": "dev" }"#;
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
    };
    let api = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Human,
        source_thread_id: None,
        emitting_trigger_id: None,
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
        caller_gateway: None,
        routing_gateway: None,
        thread_id: None,
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
        remote_start: None,
        verdict: None,
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

#[test]
fn message_origin_agent_serializes_with_its_participant() {
    let ours = MessageOrigin::Agent {
        agent: AgentParticipant::LucidosAgent,
    };
    assert_eq!(
        serde_json::to_value(&ours).unwrap(),
        serde_json::json!({"kind": "agent", "agent": {"kind": "lucidos_agent"}})
    );
    let guest = MessageOrigin::Agent {
        agent: AgentParticipant::Guest {
            label: "Voice".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(&guest).unwrap(),
        serde_json::json!({"kind": "agent", "agent": {"kind": "guest", "label": "Voice"}})
    );
}

#[test]
fn message_origin_agent_defaults_its_participant_to_the_lucidos_agent() {
    let decoded: MessageOrigin = serde_json::from_str(r#"{"kind":"agent"}"#).unwrap();
    assert_eq!(
        decoded,
        MessageOrigin::Agent {
            agent: AgentParticipant::LucidosAgent
        }
    );
    assert_eq!(AgentParticipant::default(), AgentParticipant::LucidosAgent);
}

#[test]
fn message_origin_agent_mode_is_agent() {
    let origin = MessageOrigin::Agent {
        agent: AgentParticipant::Guest {
            label: "Voice".into(),
        },
    };
    assert_eq!(origin.mode(), ActorMode::Agent);
}

#[test]
fn only_an_agent_origin_names_an_authoring_agent() {
    let authored = MessageOrigin::Agent {
        agent: AgentParticipant::LucidosAgent,
    };
    assert_eq!(authored.agent(), Some(&AgentParticipant::LucidosAgent));
    assert_eq!(MessageOrigin::System.agent(), None);
    assert_eq!(
        MessageOrigin::Device {
            device_id: "dev-1".into(),
        }
        .agent(),
        None
    );
}

#[test]
fn a_guest_speaks_under_its_own_name_and_our_agent_stays_assistant() {
    assert_eq!(AgentParticipant::LucidosAgent.speaker_label(), "Assistant");
    assert_eq!(
        AgentParticipant::Guest {
            label: "Voice".into()
        }
        .speaker_label(),
        "Voice"
    );
}

#[test]
fn a_device_origin_stores_the_id_only_and_old_rows_with_a_name_still_load() {
    let origin = MessageOrigin::Device {
        device_id: "phone-1".into(),
    };
    assert_eq!(
        serde_json::to_value(&origin).unwrap(),
        serde_json::json!({ "kind": "device", "device_id": "phone-1" })
    );
    let old: MessageOrigin =
        serde_json::from_str(r#"{"kind":"device","device_id":"phone-1","label":"device-phone-1"}"#)
            .unwrap();
    assert_eq!(old, origin);
}

#[test]
fn an_old_message_received_with_a_stored_device_name_still_loads() {
    let event: ThreadEvent = serde_json::from_value(serde_json::json!({
        "type": "MessageReceived",
        "text": "hello",
        "device_id": "phone-1",
        "device": "My iPhone\nSafari/604.1 on iOS",
        "mode": "human"
    }))
    .unwrap();
    match event {
        ThreadEvent::MessageReceived { device_id, .. } => {
            assert_eq!(device_id.as_deref(), Some("phone-1"));
        }
        other => panic!("expected MessageReceived, got {other:?}"),
    }
}

#[test]
fn only_a_verified_subprocess_names_a_source_thread() {
    let thread = uuid::Uuid::new_v4();
    let subprocess = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(thread),
        emitting_trigger_id: None,
    };
    let elsewhere = MessageOrigin::Workspace {
        workspace: "dev".to_string(),
        caller_gateway: None,
        routing_gateway: None,
        thread_id: Some(thread),
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
        remote_start: None,
        verdict: None,
    };
    let device = MessageOrigin::Device {
        device_id: "d1".to_string(),
    };
    assert_eq!(subprocess.source_thread_id(), Some(thread));
    assert_eq!(elsewhere.source_thread_id(), None);
    assert_eq!(device.source_thread_id(), None);
}
