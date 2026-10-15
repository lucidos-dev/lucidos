use serde_json::Value;

use super::{AgentParticipant, EventChannel, MessageOrigin};

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

    /// This request's meta, plus the agent that authored the event (ADR 0150).
    ///
    /// Keeps `request_event_id` and `channel`, because an authored event
    /// belongs to the same cycle as the request that prompted it.
    ///
    /// Stamp only what the agent WROTE, never the turn's request meta itself.
    /// `meta_with_cancel_actor` fills its slot only when it is empty, so a
    /// pre-filled one would attribute the user's Stop click to the agent.
    pub fn authored_by(&self, agent: AgentParticipant) -> Self {
        EventMeta {
            actor: Some(MessageOrigin::Agent { agent }),
            ..self.clone()
        }
    }

    /// Merge typed metadata fields into a JSON payload object.
    pub fn apply(&self, obj: &mut serde_json::Map<String, Value>) {
        if let Some(id) = &self.request_event_id {
            obj.insert(REQUEST_EVENT_ID_KEY.into(), Value::String(id.to_string()));
        }
        if let Some(ch) = &self.channel {
            obj.insert(
                CHANNEL_KEY.into(),
                serde_json::to_value(ch).expect("EventChannel serialization"),
            );
        }
        if let Some(actor) = &self.actor {
            obj.insert(
                ACTOR_KEY.into(),
                serde_json::to_value(actor).expect("MessageOrigin serialization"),
            );
        }
    }

    /// The metadata [`Self::apply`] merged into a stored payload. `event_id` is
    /// the row's own key, so it stays `None`. A field that does not parse
    /// reads as absent.
    pub fn from_payload(payload: &Value) -> Self {
        let field = |key: &str| payload.get(key).cloned();
        EventMeta {
            request_event_id: field(REQUEST_EVENT_ID_KEY)
                .and_then(|v| serde_json::from_value(v).ok()),
            channel: field(CHANNEL_KEY).and_then(|v| serde_json::from_value(v).ok()),
            event_id: None,
            actor: field(ACTOR_KEY).and_then(|v| serde_json::from_value(v).ok()),
        }
    }
}

const REQUEST_EVENT_ID_KEY: &str = "request_event_id";
const CHANNEL_KEY: &str = "channel";
const ACTOR_KEY: &str = "actor";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::ThreadEvent;

    #[test]
    fn a_stored_payload_gives_back_the_meta_it_was_written_with() {
        let meta = EventMeta {
            request_event_id: Some(uuid::Uuid::new_v4()),
            channel: Some(EventChannel::ClaudeCode),
            event_id: None,
            actor: Some(MessageOrigin::Device {
                device_id: "test-device".into(),
            }),
        };
        let payload = ThreadEvent::ThreadArchived.to_payload(&meta);

        let read = EventMeta::from_payload(&payload);
        assert_eq!(read.request_event_id, meta.request_event_id);
        assert_eq!(read.channel, meta.channel);
        assert_eq!(read.actor, meta.actor);
        assert_eq!(read.event_id, None);
    }

    #[test]
    fn a_payload_with_no_meta_reads_as_none() {
        let payload = ThreadEvent::ThreadArchived.to_payload(&EventMeta::NONE);
        let read = EventMeta::from_payload(&payload);
        assert_eq!(read.request_event_id, None);
        assert_eq!(read.channel, None);
        assert_eq!(read.actor, None);
    }
}
