use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventChannel {
    Chat,
    #[serde(rename = "claude_code")]
    ClaudeCode,
    #[serde(alias = "scheduled_trigger")]
    Trigger,
}

impl EventChannel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::ClaudeCode => "claude_code",
            Self::Trigger => "trigger",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "chat" => Some(Self::Chat),
            "claude_code" => Some(Self::ClaudeCode),
            "trigger" | "scheduled_trigger" => Some(Self::Trigger),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum TriggerInvocation {
    Schedule,
    /// INVARIANT: A domain event fired this run. `event_type` is the matched event name;
    /// `event_id` is the source `events.id` (when known) so the popover can
    /// deep-link back to the originating event row. `thread_id` is the thread
    /// the source event lives on (only set for thread-scoped events). It is
    /// exposed to script triggers as `TRIGGER_EVENT_THREAD_ID`. A script can
    /// then pass `--tap navigate --thread-id` (with optional `--event-id`) to
    /// `lucidos notify`, which deep-links the resulting push back to the
    /// originating conversation.
    Event {
        event_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<uuid::Uuid>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryOf {
    pub thread_id: uuid::Uuid,
    pub deadline: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FireRetry {
    Queued {
        entry_id: uuid::Uuid,
        deadline: chrono::DateTime<chrono::Utc>,
    },
    Skipped {
        reason: RetrySkipReason,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrySkipReason {
    AlreadyRetried,
    FireCompleted,
    SpawnedWork,
    TooOld,
    LaterSlotDue,
    NewerFire,
    TriggerUnavailable,
    RequestUnavailable,
    AlreadyQueued,
}

#[cfg(test)]
mod channel_tests {
    use super::*;

    const ALL_CHANNELS: &[EventChannel] = &[
        EventChannel::Chat,
        EventChannel::ClaudeCode,
        EventChannel::Trigger,
    ];

    fn exhaustive_marker(c: EventChannel) -> u8 {
        match c {
            EventChannel::Chat => 0,
            EventChannel::ClaudeCode => 1,
            EventChannel::Trigger => 2,
        }
    }

    #[test]
    fn from_wire_matches_the_serde_representation_for_every_variant() {
        let mut markers: Vec<u8> = ALL_CHANNELS
            .iter()
            .copied()
            .map(exhaustive_marker)
            .collect();
        markers.sort_unstable();
        markers.dedup();
        assert_eq!(
            markers.len(),
            ALL_CHANNELS.len(),
            "ALL_CHANNELS must list each variant exactly once"
        );

        for &channel in ALL_CHANNELS {
            let wire = serde_json::to_value(channel).unwrap();
            let wire = wire.as_str().expect("EventChannel serializes to a string");

            assert_eq!(
                wire,
                channel.as_str(),
                "as_str must match what serde writes for {channel:?}"
            );
            assert_eq!(
                EventChannel::from_wire(wire),
                Some(channel),
                "from_wire must decode the string serde writes for {channel:?}"
            );
        }
    }

    #[test]
    fn from_wire_accepts_the_legacy_trigger_alias_and_rejects_the_unknown() {
        assert_eq!(
            EventChannel::from_wire("scheduled_trigger"),
            Some(EventChannel::Trigger)
        );
        assert_eq!(EventChannel::from_wire("from_the_future"), None);
        assert_eq!(EventChannel::from_wire(""), None);
    }
}
