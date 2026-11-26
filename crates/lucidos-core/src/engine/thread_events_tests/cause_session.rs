use super::*;

#[test]
fn response_cancel_abort_cause_round_trip_and_legacy_default() {
    let legacy_canceled: ThreadEvent =
        serde_json::from_str(r#"{"type":"ResponseCanceled","text":"x"}"#).unwrap();
    match legacy_canceled {
        ThreadEvent::ResponseCanceled { cause, .. } => {
            assert_eq!(
                cause,
                CancelCause::Unknown,
                "legacy rows default to Unknown"
            )
        }
        _ => panic!("expected ResponseCanceled"),
    }
    let legacy_aborted: ThreadEvent =
        serde_json::from_str(r#"{"type":"ResponseAborted"}"#).unwrap();
    match legacy_aborted {
        ThreadEvent::ResponseAborted { cause, .. } => {
            assert_eq!(cause, AbortCause::Unknown, "legacy rows default to Unknown")
        }
        _ => panic!("expected ResponseAborted"),
    }

    let removed_cancel_cause: ThreadEvent =
        serde_json::from_str(r#"{"type":"ResponseCanceled","cause":"stale_settle"}"#).unwrap();
    match removed_cancel_cause {
        ThreadEvent::ResponseCanceled { cause, .. } => {
            assert_eq!(
                cause,
                CancelCause::Unknown,
                "removed cause strings must fall back to Unknown via #[serde(other)]"
            )
        }
        _ => panic!("expected ResponseCanceled"),
    }

    for cancel_cause in [
        CancelCause::UserStop,
        CancelCause::UserAction,
        CancelCause::Unknown,
    ] {
        let event = ThreadEvent::ResponseCanceled {
            text: "p".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: cancel_cause,
        };
        let json = serde_json::to_value(&event).unwrap();
        let round: ThreadEvent = serde_json::from_value(json.clone()).unwrap();
        match round {
            ThreadEvent::ResponseCanceled { cause, .. } => assert_eq!(cause, cancel_cause),
            _ => panic!("wrong variant"),
        }
    }
    for abort_cause in [
        AbortCause::EngineShutdown,
        AbortCause::SafetyNet,
        AbortCause::RecoveryAfterRestart,
        AbortCause::ProcessKilled,
        AbortCause::StaleSettle,
        AbortCause::Unknown,
    ] {
        let event = ThreadEvent::ResponseAborted {
            text: "p".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: abort_cause,
        };
        let json = serde_json::to_value(&event).unwrap();
        let round: ThreadEvent = serde_json::from_value(json.clone()).unwrap();
        match round {
            ThreadEvent::ResponseAborted { cause, .. } => assert_eq!(cause, abort_cause),
            _ => panic!("wrong variant"),
        }
    }
}

#[test]
fn cancel_cause_as_str_is_the_wire_name() {
    for cause in CancelCause::ALL {
        assert_eq!(
            serde_json::to_value(cause).unwrap(),
            serde_json::Value::String(cause.as_str().into()),
            "{cause:?}"
        );
    }
}

#[test]
fn session_ended_reason_serialization() {
    for (reason, expected) in [
        (SessionEndReason::Shutdown, "shutdown"),
        (SessionEndReason::Panic, "panic"),
        (SessionEndReason::Closed, "closed"),
        (SessionEndReason::StaleResume, "stale_resume"),
    ] {
        let event = ThreadEvent::SessionEnded { reason };
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized["type"], "SessionEnded");
        assert_eq!(
            serialized["reason"], expected,
            "{:?} must serialize as {:?}",
            reason, expected
        );
    }

    let old: ThreadEvent = serde_json::from_str(r#"{"type":"SessionEnded"}"#).unwrap();
    match old {
        ThreadEvent::SessionEnded { reason } => {
            assert_eq!(reason, SessionEndReason::LegacyNonTerminal)
        }
        _ => panic!("wrong variant"),
    }

    for legacy in [
        "completed",
        "user_ended",
        "changes_proposed",
        "changes_applied",
        "auto_ended",
        "discarded",
    ] {
        let raw = format!(r#"{{"type":"SessionEnded","reason":"{}"}}"#, legacy);
        let parsed: ThreadEvent = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("legacy reason {:?} should deserialize: {}", legacy, e));
        match parsed {
            ThreadEvent::SessionEnded { reason } => assert_eq!(
                reason,
                SessionEndReason::LegacyNonTerminal,
                "legacy reason {:?} should map to LegacyNonTerminal",
                legacy
            ),
            _ => panic!("wrong variant for legacy reason {:?}", legacy),
        }
    }
}

#[test]
fn abort_status_verdict_keys_on_cause_and_actor() {
    const PAUSED: &str = "'paused'";
    const FAILED: &str = "'failed'";
    const SETTLED: &str = crate::engine::event_bus::STATUS_FROM_PROPOSED_CHANGE;

    let device = MessageOrigin::Device {
        device_id: "dev-1".to_string(),
    };
    let system = MessageOrigin::system();

    for (cause, actor, expected) in [
        (AbortCause::EngineShutdown, Some(&device), PAUSED),
        (AbortCause::EngineShutdown, Some(&system), FAILED),
        (AbortCause::EngineShutdown, None, FAILED),
        (AbortCause::RecoveryAfterRestart, Some(&device), FAILED),
        (AbortCause::RecoveryAfterRestart, Some(&system), FAILED),
        (AbortCause::RecoveryAfterRestart, None, FAILED),
        (AbortCause::SafetyNet, Some(&device), FAILED),
        (AbortCause::SafetyNet, None, FAILED),
        (AbortCause::ProcessKilled, Some(&device), FAILED),
        (AbortCause::ProcessKilled, None, FAILED),
        (AbortCause::SessionDropped, Some(&device), FAILED),
        (AbortCause::SessionDropped, None, FAILED),
        (AbortCause::Unknown, Some(&device), FAILED),
        (AbortCause::Unknown, None, FAILED),
        (AbortCause::StaleSettle, Some(&device), SETTLED),
        (AbortCause::StaleSettle, Some(&system), SETTLED),
        (AbortCause::StaleSettle, None, SETTLED),
    ] {
        assert_eq!(
            cause.status_sql(actor),
            expected,
            "{:?} with actor {:?} maps to the wrong thread_summaries.status fragment",
            cause,
            actor
        );
        assert!(
            !cause.status_sql(actor).contains("waiting"),
            "{:?} with actor {:?}: an abort must not write 'waiting'. Nothing \
             preserves it, so the dying turn's drain overwrites it with \
             'running'. A pending change is the reader's business.",
            cause,
            actor
        );
    }
}

#[test]
fn paused_verdict_is_exactly_a_promised_auto_resume() {
    let device = MessageOrigin::Device {
        device_id: "dev-1".to_string(),
    };
    let system = MessageOrigin::system();

    for cause in [
        AbortCause::EngineShutdown,
        AbortCause::SafetyNet,
        AbortCause::RecoveryAfterRestart,
        AbortCause::ProcessKilled,
        AbortCause::StaleSettle,
        AbortCause::SessionDropped,
        AbortCause::Unknown,
    ] {
        for actor in [Some(&device), Some(&system), None] {
            assert_eq!(
                cause.promises_auto_resume(actor),
                cause.status_sql(actor).contains("'paused'"),
                "{:?} with actor {:?}: the resume promise and the paused verdict must agree",
                cause,
                actor
            );
        }
    }
}

#[test]
fn event_wait_cancel_causes_round_trip_including_the_retired_one() {
    for (cause, wire) in [
        (EventWaitCancelCause::UserStop, "user_stop"),
        (EventWaitCancelCause::AgentStandDown, "agent_stand_down"),
        (EventWaitCancelCause::ThreadArchived, "thread_archived"),
        (EventWaitCancelCause::ThreadDiscarded, "thread_discarded"),
        (EventWaitCancelCause::ThreadCanceled, "thread_canceled"),
    ] {
        let event = ThreadEvent::EventWaitCanceled {
            wait_id: uuid::Uuid::new_v4(),
            cause,
            on: vec![],
            reason: String::new(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(
            json["cause"], wire,
            "{cause:?} must stay on the wire as {wire:?}"
        );
        match serde_json::from_value::<ThreadEvent>(json).unwrap() {
            ThreadEvent::EventWaitCanceled { cause: back, .. } => assert_eq!(back, cause),
            _ => panic!("wrong variant"),
        }
    }

    let unknown: ThreadEvent = serde_json::from_str(
        r#"{"type":"EventWaitCanceled","wait_id":"00000000-0000-0000-0000-000000000001","cause":"who_knows"}"#,
    )
    .unwrap();
    match unknown {
        ThreadEvent::EventWaitCanceled { cause, .. } => {
            assert_eq!(cause, EventWaitCancelCause::Unknown)
        }
        _ => panic!("wrong variant"),
    }
}
