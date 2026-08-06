use super::*;

/// Pin the contract that legacy DB rows without `cause` deserialize as
/// `Unknown` rather than failing, and that fresh emissions round-trip
/// the typed cause through serde.
#[test]
fn response_cancel_abort_cause_round_trip_and_legacy_default() {
    // Legacy: no `cause` field on the wire → deserializes as Unknown.
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

    // Removed cause string: `stale_settle` was a CancelCause variant in earlier
    // builds before being moved to AbortCause. Old DB rows persisted while it
    // was a cancel cause must replay cleanly via `#[serde(other)] Unknown`,
    // not crash deserialization.
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

    // Fresh emit with typed cause survives the serde round trip in both
    // directions — the wire format uses snake_case strings.
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
fn session_ended_reason_serialization() {
    // Each emit-able variant round-trips on the wire.
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

    // Backwards compat: old DB rows without a `reason` field deserialize
    // as `LegacyNonTerminal` via the serde default.
    let old: ThreadEvent = serde_json::from_str(r#"{"type":"SessionEnded"}"#).unwrap();
    match old {
        ThreadEvent::SessionEnded { reason } => {
            assert_eq!(reason, SessionEndReason::LegacyNonTerminal)
        }
        _ => panic!("wrong variant"),
    }

    // Backwards compat: removed reasons (completed, changes_proposed,
    // changes_applied, auto_ended, user_ended, discarded) on legacy rows
    // deserialize via `#[serde(other)]` to `LegacyNonTerminal` so old data
    // doesn't crash the engine.
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

/// Every `AbortCause` paired with every kind of actor states which side of the
/// verdict split it lands on, so neither a new cause variant nor a new actor
/// case can inherit `failed` (or `paused`) by default. Adding one fails this
/// test until its status is decided deliberately.
///
/// The three outcomes: `StaleSettle` uses the cancel-style mapping (nothing was
/// running), a device-actor `EngineShutdown` is `paused` (the user's own *Switch
/// to new version*, which the engine resumes by itself), everything else is
/// `failed`. Note the two rows that make the rule narrow rather than
/// cause-shaped: `EngineShutdown` with a SYSTEM actor is `failed`, and
/// `RecoveryAfterRestart` is `failed` with EVERY actor, device included, since
/// the boot floor's promise-withdrawal exists precisely to un-promise the
/// resume.
#[test]
fn abort_status_verdict_keys_on_cause_and_actor() {
    const PAUSED: &str = "CASE WHEN coding_agent_proposed THEN 'waiting' ELSE 'paused' END";
    const FAILED: &str = "CASE WHEN coding_agent_proposed THEN 'waiting' ELSE 'failed' END";
    const SETTLED: &str = crate::engine::event_bus::STATUS_FROM_PROPOSED_CHANGE;

    let device = MessageOrigin::Device {
        device_id: "dev-1".to_string(),
        label: "My MacBook".to_string(),
    };
    let system = MessageOrigin::system();

    for (cause, actor, expected) in [
        // The one shape the engine promised to resume.
        (AbortCause::EngineShutdown, Some(&device), PAUSED),
        (AbortCause::EngineShutdown, Some(&system), FAILED),
        (AbortCause::EngineShutdown, None, FAILED),
        // The crash boundary, and the boot floor withdrawing a resume promise.
        (AbortCause::RecoveryAfterRestart, Some(&device), FAILED),
        (AbortCause::RecoveryAfterRestart, Some(&system), FAILED),
        (AbortCause::RecoveryAfterRestart, None, FAILED),
        // Real failures, whoever happened to be attributed.
        (AbortCause::SafetyNet, Some(&device), FAILED),
        (AbortCause::SafetyNet, None, FAILED),
        (AbortCause::ProcessKilled, Some(&device), FAILED),
        (AbortCause::ProcessKilled, None, FAILED),
        (AbortCause::SessionDropped, Some(&device), FAILED),
        (AbortCause::SessionDropped, None, FAILED),
        (AbortCause::Unknown, Some(&device), FAILED),
        (AbortCause::Unknown, None, FAILED),
        // Cleanup of a row whose process was already gone. The device actor is
        // the button that exposed it (Stop / Apply / Discard / Archive), which
        // is exactly why a device actor alone cannot mean "switch".
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
    }
}

/// The `paused` verdict is DERIVED from `promises_auto_resume()`, not from a
/// second list that could drift out of step with it. `paused` and "the engine
/// will bring this turn back" must be the same statement, because the frontend
/// withholds the Continue button on that same predicate: any drift tells the
/// user the opposite of what they can do.
#[test]
fn paused_verdict_is_exactly_a_promised_auto_resume() {
    let device = MessageOrigin::Device {
        device_id: "dev-1".to_string(),
        label: "My MacBook".to_string(),
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
        // `StaleSettle` is in the loop rather than skipped: it writes no verdict
        // at all, so BOTH sides are false and the equivalence holds trivially.
        // Excluding it would be excluding the one cause a device actor makes most
        // tempting to read as a switch.
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
