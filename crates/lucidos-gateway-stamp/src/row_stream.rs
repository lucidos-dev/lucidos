use serde::{Deserialize, Serialize};

pub const PATH: &str = "/api/v1/events";

pub const AGGREGATES_PARAM: &str = "aggregates";

pub const HEALTH_FIELD: &str = "event_filter";

pub const AGGREGATES: [&str; 4] = ["notification", "ops", "preference", "user"];

pub const UNREAD_EVENT_TYPES: [&str; 5] = [
    "NotificationCreated",
    "NotificationRead",
    "NotificationsAllRead",
    "ThreadsDeleted",
    "UsersChanged",
];

pub const BACKUP_EVENT_TYPES: [&str; 3] = ["BackupCompleted", "BackupFailed", "PreferencesChanged"];

pub const SILENCE_LIMIT: std::time::Duration = std::time::Duration::from_secs(90);

pub const RESYNC_FRAMES: [&str; 2] = ["StreamHello", "Lagged"];

pub fn request_path() -> String {
    format!("{PATH}?{AGGREGATES_PARAM}={}", AGGREGATES.join(","))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poke {
    Resync,
    Unread,
    Backup,
}

impl Poke {
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
    pub fn for_event(event_type: &str, aggregate: &str, wanted: &[String]) -> Option<Self> {
        Poke::of_event(event_type)?;
        wanted.iter().any(|w| w == aggregate).then(|| Self {
            event_type: event_type.to_string(),
            aggregate: Some(aggregate.to_string()),
        })
    }

    pub fn poke(&self) -> Option<Poke> {
        let event_type = self.event_type.as_str();
        if RESYNC_FRAMES.contains(&event_type) {
            return Some(Poke::Resync);
        }
        Poke::of_event(event_type)
    }
}

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
