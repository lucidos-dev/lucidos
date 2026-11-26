use serde_json::Value;

use super::{AgentParticipant, EventChannel, MessageOrigin};

#[derive(Clone, Debug, Default)]
pub struct EventMeta {
    pub request_event_id: Option<uuid::Uuid>,
    pub channel: Option<EventChannel>,
    pub event_id: Option<uuid::Uuid>,
    pub actor: Option<MessageOrigin>,
    pub queued_for_permission: bool,
}

impl EventMeta {
    pub const NONE: EventMeta = EventMeta {
        request_event_id: None,
        channel: None,
        event_id: None,
        actor: None,
        queued_for_permission: false,
    };

    pub fn with_actor(actor: Option<MessageOrigin>) -> Self {
        EventMeta {
            actor,
            ..EventMeta::NONE
        }
    }

    pub fn authored_by(&self, agent: AgentParticipant) -> Self {
        EventMeta {
            actor: Some(MessageOrigin::Agent { agent }),
            ..self.clone()
        }
    }

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
        if self.queued_for_permission {
            obj.insert("queued_for_permission".into(), Value::Bool(true));
        }
        if let Some(actor) = &self.actor {
            obj.insert(
                ACTOR_KEY.into(),
                serde_json::to_value(actor).expect("MessageOrigin serialization"),
            );
        }
    }

    pub fn from_payload(payload: &Value) -> Self {
        let field = |key: &str| payload.get(key).cloned();
        EventMeta {
            request_event_id: field(REQUEST_EVENT_ID_KEY)
                .and_then(|v| serde_json::from_value(v).ok()),
            channel: field(CHANNEL_KEY).and_then(|v| serde_json::from_value(v).ok()),
            event_id: None,
            actor: field(ACTOR_KEY).and_then(|v| serde_json::from_value(v).ok()),
            queued_for_permission: payload["queued_for_permission"].as_bool().unwrap_or(false),
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
            queued_for_permission: true,
        };
        let payload = ThreadEvent::ThreadArchived.to_payload(&meta);

        let read = EventMeta::from_payload(&payload);
        assert_eq!(read.request_event_id, meta.request_event_id);
        assert_eq!(read.channel, meta.channel);
        assert_eq!(read.actor, meta.actor);
        assert_eq!(read.event_id, None);
        assert!(read.queued_for_permission);
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
