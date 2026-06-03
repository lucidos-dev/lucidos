use serde_json::Value;

use super::{EventChannel, MessageOrigin};

/// Cross-cutting metadata merged into the event payload during persistence.
/// Not part of the ThreadEvent type itself — these are routing/grouping fields
/// used by read queries (message grouping, channel filtering).
#[derive(Clone, Debug, Default)]
pub struct EventMeta {
    /// Links response events back to the request that triggered them.
    pub request_event_id: Option<uuid::Uuid>,
    /// Source channel. Always set on origin events.
    pub channel: Option<EventChannel>,
    /// Client-provided UUID to use as the DB event primary key.
    /// If set, EventBus uses this instead of generating a new UUID.
    /// Allows frontend to match SSE events back to pending messages.
    pub event_id: Option<uuid::Uuid>,
    /// Audit: who initiated. Mutating handlers stamp via `api/actor::user_actor`;
    /// internal state machines leave None.
    pub actor: Option<MessageOrigin>,
}

impl EventMeta {
    pub const NONE: EventMeta = EventMeta {
        request_event_id: None,
        channel: None,
        event_id: None,
        actor: None,
    };

    pub fn with_actor(actor: Option<MessageOrigin>) -> Self {
        EventMeta {
            actor,
            ..EventMeta::NONE
        }
    }

    /// Merge typed metadata fields into a JSON payload object.
    pub fn apply(&self, obj: &mut serde_json::Map<String, Value>) {
        if let Some(id) = &self.request_event_id {
            obj.insert("request_event_id".into(), Value::String(id.to_string()));
        }
        if let Some(ch) = &self.channel {
            obj.insert(
                "channel".into(),
                serde_json::to_value(ch).expect("EventChannel serialization"),
            );
        }
        if let Some(actor) = &self.actor {
            obj.insert(
                "actor".into(),
                serde_json::to_value(actor).expect("MessageOrigin serialization"),
            );
        }
    }
}
