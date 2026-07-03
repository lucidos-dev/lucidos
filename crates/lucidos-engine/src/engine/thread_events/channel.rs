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
