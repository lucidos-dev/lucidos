//! The row stream: the filtered event stream a gateway holds to each engine,
//! to keep that workspace's picker row current (ADR 0756).
//!
//! The engine offers it and the gateway reads it, so both take these names
//! from here. The engine's event types and aggregates are defined in
//! `lucidos-core`, which the gateway cannot depend on. A test beside the
//! engine's stream handler pins every name below to them.

use serde::{Deserialize, Serialize};

/// The engine route that serves the stream.
pub const PATH: &str = "/api/v1/events";

/// The query parameter that selects the filtered mode: a comma-separated list
/// of aggregates.
pub const AGGREGATES_PARAM: &str = "aggregates";

/// The health field an engine that offers the filtered mode sets to `true`.
/// An engine without it ignores the parameter and would count the gateway as
/// a page, so a gateway never subscribes to one.
pub const HEALTH_FIELD: &str = "event_filter";

/// The aggregates a gateway asks for.
pub const AGGREGATES: [&str; 4] = ["notification", "ops", "preference", "user"];

/// The event types that move an unread count: the shared one, or the one a
/// single reader sees. A deleted thread takes its notifications with it, and
/// a users change can move a device to another reader.
pub const UNREAD_EVENT_TYPES: [&str; 5] = [
    "NotificationCreated",
    "NotificationRead",
    "NotificationsAllRead",
    "ThreadsDeleted",
    "UsersChanged",
];

/// The event types that move the backup line.
pub const BACKUP_EVENT_TYPES: [&str; 3] = ["BackupCompleted", "BackupFailed", "PreferencesChanged"];

/// How long a reader lets the stream stay silent before it drops it and
/// subscribes again. The engine sends a keepalive in every quiet interval, so
/// this much silence means the stream stalled. It is three of the engine's
/// keepalive intervals, and a test beside the engine's handler pins that.
pub const SILENCE_LIMIT: std::time::Duration = std::time::Duration::from_secs(90);

/// The stream's control frames. Each means the reader may have missed
/// events, so it reads its snapshot again.
pub const RESYNC_FRAMES: [&str; 2] = ["StreamHello", "Lagged"];

/// The path and query a gateway requests.
pub fn request_path() -> String {
    format!("{PATH}?{AGGREGATES_PARAM}={}", AGGREGATES.join(","))
}

/// One frame of the filtered mode: an event's type and aggregate, and never
/// its payload. A reader takes it as a poke and reads the value itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    #[serde(rename = "type")]
    pub event_type: String,
    /// Absent on a control frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<String>,
}

/// What a reader does with one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poke {
    /// Events may be missing, so nothing held is trusted.
    Resync,
    /// An unread count moved, so no held count is trusted.
    Unread,
    /// The backup line moved.
    Backup,
}

impl Poke {
    /// The poke an event of `event_type` is, or `None` for a type the stream
    /// never sends. Sharing an aggregate with a sent type is not enough.
    fn of_event(event_type: &str) -> Option<Self> {
        if UNREAD_EVENT_TYPES.contains(&event_type) {
            Some(Self::Unread)
        } else {
            BACKUP_EVENT_TYPES
                .contains(&event_type)
                .then_some(Self::Backup)
        }
    }
}

impl Frame {
    /// The frame for an event of `event_type` under `aggregate`, when the
    /// caller asked for that aggregate and the type is one the stream sends.
    pub fn for_event(event_type: &str, aggregate: &str, wanted: &[String]) -> Option<Self> {
        Poke::of_event(event_type)?;
        wanted.iter().any(|w| w == aggregate).then(|| Self {
            event_type: event_type.to_string(),
            aggregate: Some(aggregate.to_string()),
        })
    }

    /// `None` for a frame the reader ignores: a type the stream never sends.
    pub fn poke(&self) -> Option<Poke> {
        let event_type = self.event_type.as_str();
        if RESYNC_FRAMES.contains(&event_type) {
            return Some(Poke::Resync);
        }
        Poke::of_event(event_type)
    }
}

/// The aggregates named by an `aggregates` query value.
pub fn parse_aggregates(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted() -> Vec<String> {
        parse_aggregates(&AGGREGATES.join(","))
    }

    #[test]
    fn a_frame_carries_the_type_and_the_aggregate_and_nothing_else() {
        let frame = Frame::for_event("NotificationRead", "notification", &wanted()).unwrap();
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            serde_json::json!({"type": "NotificationRead", "aggregate": "notification"})
        );
    }

    #[test]
    fn sharing_an_aggregate_does_not_pass_an_event() {
        for (event_type, aggregate) in [
            ("NativePushRequested", "notification"),
            ("BackupKeyRevealed", "ops"),
            ("ThreadArchived", "ops"),
            ("DeviceRenamed", "user"),
            ("LanguageSet", "preference"),
        ] {
            assert_eq!(Frame::for_event(event_type, aggregate, &wanted()), None);
        }
    }

    #[test]
    fn an_aggregate_the_caller_did_not_name_is_dropped() {
        let only_ops = parse_aggregates("ops");
        assert_eq!(
            Frame::for_event("NotificationRead", "notification", &only_ops),
            None
        );
        assert!(Frame::for_event("BackupCompleted", "ops", &only_ops).is_some());
        assert_eq!(
            Frame::for_event("BackupCompleted", "ops", &parse_aggregates("")),
            None
        );
    }

    #[test]
    fn a_reader_pokes_on_control_frames_and_accepted_events_only() {
        let frame = |event_type: &str, aggregate: Option<&str>| Frame {
            event_type: event_type.into(),
            aggregate: aggregate.map(str::to_string),
        };
        assert_eq!(frame("StreamHello", None).poke(), Some(Poke::Resync));
        assert_eq!(frame("Lagged", None).poke(), Some(Poke::Resync));
        assert_eq!(
            frame("NotificationCreated", Some("notification")).poke(),
            Some(Poke::Unread)
        );
        // A deleted thread rides `ops`, and still moves the unread count.
        assert_eq!(
            frame("ThreadsDeleted", Some("ops")).poke(),
            Some(Poke::Unread)
        );
        assert_eq!(
            frame("UsersChanged", Some("user")).poke(),
            Some(Poke::Unread)
        );
        assert_eq!(
            frame("BackupFailed", Some("ops")).poke(),
            Some(Poke::Backup)
        );
        assert_eq!(frame("ThreadEvent", None).poke(), None);
        assert_eq!(
            frame("NativePushRequested", Some("notification")).poke(),
            None
        );
    }

    #[test]
    fn the_request_names_every_aggregate() {
        assert_eq!(
            request_path(),
            "/api/v1/events?aggregates=notification,ops,preference,user"
        );
    }
}
