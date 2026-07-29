use serde::{Deserialize, Serialize};

/// Source channel for events — determines thread type and routing.
///
/// Variant names track the *coding agent product* the channel belongs to,
/// not the role. `ClaudeCode` serializes to `"claude_code"` — the wire
/// string is the deliberate Claude-Code instance identifier (not a legacy
/// alias) and is part of the persistence + frontend contract. A future
/// Codex coding agent would slot in as `EventChannel::Codex` with wire
/// string `"codex"`; see the *Channel* dev-glossary entry.
///
/// `Trigger` is the umbrella for all trigger-driven runs (scheduled, event,
/// hybrid). The actual invocation that fired a given run is recorded on
/// `TriggerStarted.invocation`.
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

    /// Parse the wire `payload->>'channel'` string back into the typed enum.
    /// Mirror of [`Self::as_str`] + serde's snake_case alias `scheduled_trigger`
    /// (kept for legacy DB rows). Returns `None` for unknown variants so
    /// callers can fall back rather than panic.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "chat" => Some(Self::Chat),
            "claude_code" => Some(Self::ClaudeCode),
            "trigger" | "scheduled_trigger" => Some(Self::Trigger),
            _ => None,
        }
    }
}

/// Records *which path* fired a particular trigger run.
///
/// A trigger config can have cron-only (`schedule`), event-only (`event`), or
/// both (`hybrid`). When the scheduler dispatches a run it knows exactly which
/// path won — this enum captures that for the popover panel and any consumer
/// that wants to reason about the actual invocation rather than the config
/// shape.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum TriggerInvocation {
    /// Cron schedule fired this run.
    Schedule,
    /// A domain event fired this run. `event_type` is the matched event name;
    /// `event_id` is the source `events.id` (when known) so the popover can
    /// deep-link back to the originating event row. `thread_id` is the thread
    /// the source event lives on (only set for thread-scoped events) — exposed
    /// to script triggers as `TRIGGER_EVENT_THREAD_ID` so a script can pass
    /// `--tap navigate --thread-id` (with optional `--event-id`) to
    /// `lucidos notify` and deep-link the resulting push back to the
    /// originating conversation.
    Event {
        event_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<uuid::Uuid>,
    },
}

#[cfg(test)]
mod channel_tests {
    use super::*;

    /// Every variant, as the parity test's input set. Kept in sync with the enum
    /// by `exhaustive_marker` below — adding a variant breaks that match, and the
    /// compiler error points here.
    const ALL_CHANNELS: &[EventChannel] = &[
        EventChannel::Chat,
        EventChannel::ClaudeCode,
        EventChannel::Trigger,
    ];

    /// Compile-time guard: a new `EventChannel` variant fails to compile here,
    /// forcing whoever adds it to also add it to `ALL_CHANNELS` above so the
    /// parity test below actually covers it.
    fn exhaustive_marker(c: EventChannel) -> u8 {
        match c {
            EventChannel::Chat => 0,
            EventChannel::ClaudeCode => 1,
            EventChannel::Trigger => 2,
        }
    }

    /// `from_wire` is a hand-rolled match, so it can silently drift from the
    /// serde representation that actually writes the wire strings — a new variant
    /// would deserialize fine but `from_wire` would return `None`, and every
    /// caller (`chat::recovery`'s orphan sweep, `chat::rerun`'s resume-channel
    /// resolution) silently falls back to `Chat`. For the resume path that
    /// mislabels the resumed turn AND rewrites the thread's `source`, because the
    /// `ContinuationStarted` projection arm stores the channel as `source`.
    /// This pins the two together instead.
    #[test]
    fn from_wire_matches_the_serde_representation_for_every_variant() {
        let mut markers: Vec<u8> = ALL_CHANNELS.iter().copied().map(exhaustive_marker).collect();
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

    /// The legacy spelling kept as a serde alias must also survive `from_wire`,
    /// or an old trigger row resumes on the wrong channel.
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
