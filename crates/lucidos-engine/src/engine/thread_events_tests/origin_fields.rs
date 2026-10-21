use super::*;

#[test]
fn continuation_started_event_can_carry_engine_origin() {
    let event = ThreadEvent::ContinuationStarted {
        branch: "claude-code/20260422".into(),
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::ContinuationStarted,
        }),
        reason: None,
        resumed_abort_id: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ContinuationStarted");
    assert_eq!(json["origin"]["kind"], "engine");
    assert_eq!(json["origin"]["reason"]["kind"], "continuation_started");
}

#[test]
fn continuation_started_event_origin_defaults_to_none_when_missing() {
    // Old DB rows without origin must deserialize cleanly.
    let json = r#"{"type":"ContinuationStarted","branch":"claude-code/20260318"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::ContinuationStarted { branch, origin, .. } => {
            assert_eq!(branch, "claude-code/20260318");
            assert!(origin.is_none());
        }
        other => panic!("expected ContinuationStarted, got {:?}", other),
    }
}

#[test]
fn continuation_started_event_accepts_legacy_session_recovered_alias() {
    // Old DB rows persisted as SessionRecovered must still deserialize as the
    // renamed ContinuationStarted variant. Also covers the older SessionResumed
    // name (renamed → SessionRecovered in 2026-03-20, then →
    // ContinuationStarted in 2026-05-13). Without the serde aliases the
    // projection blows up on every historical row.
    for legacy in &[
        r#"{"type":"SessionRecovered","branch":"claude-code/legacy"}"#,
        r#"{"type":"SessionResumed","branch":"claude-code/older-legacy"}"#,
    ] {
        let parsed: ThreadEvent = serde_json::from_str(legacy)
            .unwrap_or_else(|e| panic!("legacy {} should deserialize: {}", legacy, e));
        match parsed {
            ThreadEvent::ContinuationStarted { .. } => {}
            other => panic!(
                "expected ContinuationStarted from {}, got {:?}",
                legacy, other
            ),
        }
    }
}

#[test]
fn change_proposed_event_can_carry_engine_origin() {
    let event = ThreadEvent::ChangeProposed {
        change_id: "abc".into(),
        description: Some("stale session cleanup".into()),
        files: vec!["src/main.rs".into()],
        requires_restart: false,
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::StaleSession,
        }),
        commit_sha: None,
        branch_name: String::new(),
        repo_root: String::new(),
        hardened: false,
        incomplete: false,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ChangeProposed");
    assert_eq!(json["origin"]["kind"], "engine");
    assert_eq!(json["origin"]["reason"]["kind"], "stale_session");
}

#[test]
fn change_proposed_event_origin_defaults_to_none_when_missing() {
    // Old DB rows without origin must deserialize cleanly.
    let json = r#"{"type":"ChangeProposed","change_id":"x","description":"y","files":[],"requires_restart":false}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::ChangeProposed {
            change_id, origin, ..
        } => {
            assert_eq!(change_id, "x");
            assert!(origin.is_none());
        }
        other => panic!("expected ChangeProposed, got {:?}", other),
    }
}

#[test]
fn change_proposed_event_can_carry_orphan_recovery_origin() {
    let event = ThreadEvent::ChangeProposed {
        change_id: "def".into(),
        description: Some("orphan cleanup".into()),
        files: vec!["src/lib.rs".into()],
        requires_restart: false,
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::OrphanRecovery,
        }),
        commit_sha: None,
        branch_name: String::new(),
        repo_root: String::new(),
        hardened: false,
        incomplete: false,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["origin"]["reason"]["kind"], "orphan_recovery");
}

#[test]
fn coding_agent_prompt_sent_can_carry_orphan_recovery_origin() {
    let event = ThreadEvent::CodingAgentPromptSent {
        input_event_id: None,
        text: "resume after restart".into(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::OrphanRecovery,
        }),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "CodingAgentPromptSent");
    assert_eq!(json["origin"]["kind"], "engine");
    assert_eq!(json["origin"]["reason"]["kind"], "orphan_recovery");
}

#[test]
fn coding_agent_prompt_sent_can_carry_harden_retrigger_origin() {
    let event = ThreadEvent::CodingAgentPromptSent {
        input_event_id: None,
        text: "Run /harden now.".into(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::HardenRetrigger,
        }),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "CodingAgentPromptSent");
    assert_eq!(json["origin"]["kind"], "engine");
    assert_eq!(json["origin"]["reason"]["kind"], "harden_retrigger");
}

#[test]
fn coding_agent_prompt_sent_origin_defaults_to_none_when_missing() {
    let json = r#"{"type":"CodingAgentPromptSent","text":"hi"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::CodingAgentPromptSent { text, origin, .. } => {
            assert_eq!(text, "hi");
            assert!(origin.is_none());
        }
        other => panic!("expected CodingAgentPromptSent, got {:?}", other),
    }
}

#[test]
fn trigger_started_can_carry_scheduler_origin() {
    let id = uuid::Uuid::new_v4().to_string();
    let event = ThreadEvent::TriggerStarted {
        provider: None,
        trigger_id: id.clone(),
        trigger_name: Some("nightly".into()),
        prompt: Some("run".into()),
        invocation: Some(TriggerInvocation::Schedule),
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::Scheduler {
                trigger_id: id.clone(),
                trigger_name: Some("nightly".into()),
            },
        }),
        go_to_review: false,
        model: None,
        reasoning_effort: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "TriggerStarted");
    assert_eq!(json["origin"]["reason"]["kind"], "scheduler");
    assert_eq!(json["origin"]["reason"]["trigger_id"], id);
    assert_eq!(json["origin"]["reason"]["trigger_name"], "nightly");
}

#[test]
fn trigger_started_origin_defaults_to_none_when_missing() {
    let json = r#"{"type":"TriggerStarted","trigger_id":"id"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::TriggerStarted { origin, .. } => assert!(origin.is_none()),
        other => panic!("expected TriggerStarted, got {:?}", other),
    }
}

// ---- `mode` field deserialization for MessageReceived ----

#[test]
fn message_received_mode_field_deserializes() {
    let json = r#"{"type":"MessageReceived","text":"hi","mode":"engine"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::MessageReceived { mode, .. } => assert_eq!(mode, ActorMode::Engine),
        other => panic!("expected MessageReceived, got {:?}", other),
    }
}

/// Legacy DB rows predating the `mode` field must replay as `Human` so
/// historical events keep loading. New emissions are forced by the API
/// layer to set `mode` explicitly.
#[test]
fn message_received_no_mode_defaults_to_human() {
    let json = r#"{"type":"MessageReceived","text":"hi"}"#;
    let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
    match parsed {
        ThreadEvent::MessageReceived { mode, .. } => assert_eq!(mode, ActorMode::Human),
        other => panic!("expected MessageReceived, got {:?}", other),
    }
}

// ---- the cause of a plugin setup update ----

fn setup_occasion(json: &str) -> PluginSetupOccasion {
    serde_json::from_str(json).unwrap()
}

/// Every `plugin_setup` origin stored before the cause existed means the
/// instructions changed, so it must decode to exactly that.
#[test]
fn a_legacy_plugin_setup_update_decodes_as_instructions_changed() {
    for json in [
        r#"{"kind":"update","from_version":"0.1.3"}"#,
        r#"{"kind":"update"}"#,
    ] {
        match setup_occasion(json) {
            PluginSetupOccasion::Update { cause, .. } => {
                assert_eq!(cause, PluginSetupCause::InstructionsChanged, "{json}")
            }
            other => panic!("expected an update, got {other:?}"),
        }
    }
}

#[test]
fn a_plugin_setup_cause_round_trips_with_its_revision_and_reason() {
    for cause in [
        PluginSetupCause::InstructionsChanged,
        PluginSetupCause::RevisionRaised {
            revision: 2,
            reason: Some("The skill changed.".into()),
        },
        PluginSetupCause::RevisionRaised {
            revision: 2,
            reason: None,
        },
        PluginSetupCause::Both {
            revision: 3,
            reason: Some("Moved the install folder.".into()),
        },
    ] {
        let occasion = PluginSetupOccasion::Update {
            from_version: Some("0.1.0".into()),
            cause,
        };
        let back: PluginSetupOccasion =
            serde_json::from_value(serde_json::to_value(&occasion).unwrap()).unwrap();
        assert_eq!(back, occasion);
    }
}

#[test]
fn a_plugin_setup_revision_cause_uses_snake_case_kinds() {
    let json = serde_json::to_value(PluginSetupCause::RevisionRaised {
        revision: 2,
        reason: Some("Why.".into()),
    })
    .unwrap();
    assert_eq!(json["kind"], "revision_raised");
    assert_eq!(json["revision"], 2);
    assert_eq!(json["reason"], "Why.");
}
