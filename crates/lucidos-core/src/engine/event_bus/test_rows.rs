//! Rows and frames the event-bus tests share.

use chrono::Utc;
use uuid::Uuid;

use super::{BusEvent, EmittedEvent, EventBus, SystemEvent};

/// A workspace domain event, as a build check might emit it.
pub(super) fn build_observed() -> SystemEvent {
    SystemEvent::DomainEvent {
        event_type: "BuildObserved".into(),
        payload: serde_json::json!({"summary": "green"}),
        depth: 0,
        transient: false,
        actor: None,
    }
}

/// Emit [`build_observed`] and return the sequence its row drew.
pub(super) async fn emit_row(bus: &EventBus) -> i64 {
    bus.emit(BusEvent::System(build_observed()))
        .await
        .unwrap()
        .expect("persisted")
        .seq
}

/// A frame carrying the row at `seq`.
pub(super) fn row_frame(seq: i64) -> EmittedEvent {
    EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(seq),
        created: Utc::now(),
        typed: BusEvent::System(build_observed()),
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    }
}
