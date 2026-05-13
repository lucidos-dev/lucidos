use crate::engine::thread_events::{EngineReason, MessageOrigin, ThreadEvent};

#[test]
fn merge_conflict_event_serializes_with_engine_origin() {
    let ev = ThreadEvent::MergeConflictDetected {
        change_id: "cid".to_string(),
        files: vec!["a.rs".to_string()],
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::MergeConflict,
        }),
    };
    let v = serde_json::to_value(&ev).expect("serializes");
    assert_eq!(v["origin"]["kind"], "engine");
    assert_eq!(v["origin"]["reason"]["kind"], "merge_conflict");
}

#[test]
fn missing_hardening_event_serializes_with_engine_origin() {
    let ev = ThreadEvent::MissingHardeningDetected {
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::MissingHardening,
        }),
    };
    let v = serde_json::to_value(&ev).expect("serializes");
    assert_eq!(v["origin"]["kind"], "engine");
    assert_eq!(v["origin"]["reason"]["kind"], "missing_hardening");
}
