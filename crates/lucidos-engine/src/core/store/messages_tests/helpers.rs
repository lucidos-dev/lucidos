use super::super::types::*;
pub(super) use crate::core::EventRow;
pub(super) use chrono::{TimeZone, Utc};
pub(super) use serde_json::json;
pub(super) use uuid::Uuid;

pub(super) fn make_event(event_type: &str, payload: serde_json::Value, secs: i64) -> EventRow {
    EventRow {
        id: Uuid::new_v4(),
        event_type: event_type.to_string(),
        payload,
        created: Utc.timestamp_opt(1700000000 + secs, 0).unwrap(),
        thread_id: None,
        sequence: None,
    }
}

// Helper: generate a text segment of exactly `n` chars padded with dots
pub(super) fn pad(prefix: &str, n: usize) -> String {
    let mut s = prefix.to_string();
    while s.len() < n {
        s.push('.');
    }
    s.truncate(n);
    s
}

/// Helper: count text events in a ResponseEvent vec
pub(super) fn text_events(events: &[ResponseEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::Text { md } => Some(md.as_str()),
            _ => None,
        })
        .collect()
}

/// Helper: count step events in a ResponseEvent vec
pub(super) fn step_events(events: &[ResponseEvent]) -> Vec<(&str, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::Step {
                description,
                success,
                ..
            } => Some((description.as_str(), *success)),
            _ => None,
        })
        .collect()
}

pub(super) fn tool_event(event_type: &str, name: &str, secs: i64) -> EventRow {
    EventRow {
        id: Uuid::new_v4(),
        event_type: event_type.to_string(),
        payload: serde_json::json!({"name": name}),
        created: Utc.timestamp_opt(1700000000 + secs, 0).unwrap(),
        thread_id: None,
        sequence: None,
    }
}
