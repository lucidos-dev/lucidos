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

/// Every `AbortCause` states which side of the verdict split it lands on, so a
/// new variant cannot inherit `failed` (or `paused`) by default. Adding one
/// fails this test until its status is decided deliberately.
///
/// The three outcomes: `StaleSettle` uses the cancel-style mapping (nothing was
/// running), a TRANSIENT cause is `paused` (an engine restart interrupted the
/// turn and either resumes it or offers Continue), everything else is `failed`.
#[test]
fn abort_cause_status_sql_splits_transient_from_failed() {
    const PAUSED: &str = "CASE WHEN coding_agent_proposed THEN 'waiting' ELSE 'paused' END";
    const FAILED: &str = "CASE WHEN coding_agent_proposed THEN 'waiting' ELSE 'failed' END";

    for (cause, expected) in [
        (AbortCause::EngineShutdown, PAUSED),
        (AbortCause::RecoveryAfterRestart, PAUSED),
        (AbortCause::SafetyNet, FAILED),
        (AbortCause::ProcessKilled, FAILED),
        (AbortCause::SessionDropped, FAILED),
        (AbortCause::Unknown, FAILED),
        (
            AbortCause::StaleSettle,
            crate::engine::event_bus::STATUS_FROM_PROPOSED_CHANGE,
        ),
    ] {
        assert_eq!(
            cause.status_sql(),
            expected,
            "{:?} maps to the wrong thread_summaries.status fragment",
            cause
        );
    }
}

/// The verdict split is DERIVED from `is_transient()`, not from a second list
/// that could drift out of step with it. A cause the enum calls transient (a
/// fresh `SessionStarted` is expected) must never surface the red error dot.
#[test]
fn transient_abort_causes_are_exactly_the_paused_ones() {
    for cause in [
        AbortCause::EngineShutdown,
        AbortCause::SafetyNet,
        AbortCause::RecoveryAfterRestart,
        AbortCause::ProcessKilled,
        AbortCause::StaleSettle,
        AbortCause::SessionDropped,
        AbortCause::Unknown,
    ] {
        if cause == AbortCause::StaleSettle {
            continue; // Settles a row whose process was already gone: no verdict.
        }
        assert_eq!(
            cause.is_transient(),
            cause.status_sql().contains("'paused'"),
            "{:?}: transience and the paused verdict must agree",
            cause
        );
    }
}
