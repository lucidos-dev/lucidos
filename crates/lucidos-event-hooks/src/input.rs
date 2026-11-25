//! What a hook reads: the pending event, as the event store would record it,
//! with every stored secret masked.

use lucidos_core::core::event_subscription::{matchable_system_payload, matchable_thread_payload};
use lucidos_core::core::redact_subprocess_output_in_json;
use lucidos_core::engine::event_bus::BusEvent;
use serde::Serialize;
use serde_json::Value;

/// An event about to be recorded, in the view a hook's `condition` matches:
/// the stored payload plus the thread it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingEvent {
    pub event_type: String,
    pub payload: Value,
}

impl PendingEvent {
    pub fn of(event: &BusEvent) -> Self {
        match event {
            BusEvent::Thread {
                thread_id,
                event,
                meta,
            } => Self {
                event_type: event.event_type().to_string(),
                payload: matchable_thread_payload(event, meta, *thread_id),
            },
            BusEvent::System(event) => Self {
                event_type: event.stored_event_type().to_string(),
                payload: matchable_system_payload(event),
            },
        }
    }
}

/// A [`PendingEvent`] fit to hand a hook script. Its fields are private, so
/// the only way to build one masks the payload first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HookInput {
    event_type: String,
    payload: Value,
}

impl HookInput {
    /// Masks `pending` against every secret the workspace stores. An `Err`
    /// means the secrets could not be read, so no input exists to hand on.
    pub async fn read(pool: &sqlx::PgPool, pending: &PendingEvent) -> Result<Self, String> {
        let secrets = lucidos_engine::engine::engine_impl::stored_secret_values(pool)
            .await
            .map_err(|e| format!("the stored secrets to mask could not be read: {e}"))?;
        Ok(Self::masked(pending, &secrets))
    }

    pub(crate) fn masked(pending: &PendingEvent, secrets: &[String]) -> Self {
        let mut payload = pending.payload.clone();
        redact_subprocess_output_in_json(&mut payload, secrets);
        Self {
            event_type: pending.event_type.clone(),
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_core::core::{AuthType, CredentialStore, REDACTED_MARKER};
    use lucidos_core::engine::event_bus::{EventBus, SystemEvent};
    use lucidos_core::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
    use serde_json::json;

    const SECRET: &str = "gh-secret-value-0123456789";

    fn note_event(value: &str) -> BusEvent {
        BusEvent::System(SystemEvent::EnvironmentVariableSet {
            name: "NOTE".to_string(),
            value: value.to_string(),
            actor: None,
        })
    }

    #[test]
    fn a_thread_event_carries_its_thread_and_actor() {
        let thread_id = uuid::Uuid::new_v4();
        let pending = PendingEvent::of(&BusEvent::Thread {
            thread_id,
            event: ThreadEvent::TriggerCompleted {
                trigger_id: "nightly".to_string(),
                trigger_name: None,
                result_summary: None,
            },
            meta: EventMeta::with_actor(Some(MessageOrigin::Device {
                device_id: "my-macbook".to_string(),
            })),
        });
        assert_eq!(pending.event_type, "TriggerCompleted");
        assert_eq!(pending.payload["thread_id"], json!(thread_id.to_string()));
        assert_eq!(pending.payload["trigger_id"], json!("nightly"));
        assert_eq!(pending.payload["actor"]["device_id"], json!("my-macbook"));
    }

    #[test]
    fn a_system_event_names_its_own_fields() {
        let pending = PendingEvent::of(&note_event("hello"));
        assert_eq!(pending.event_type, "EnvironmentVariableSet");
        assert_eq!(pending.payload, json!({"name": "NOTE", "value": "hello"}));
    }

    #[test]
    fn the_input_masks_a_secret_at_any_depth() {
        let pending = PendingEvent {
            event_type: "MessageReceived".to_string(),
            payload: json!({
                "text": format!("use {SECRET} now"),
                "nested": [{"key": SECRET}],
                "keyed": {SECRET: true},
            }),
        };
        let input =
            serde_json::to_string(&HookInput::masked(&pending, &[SECRET.to_string()])).unwrap();
        assert!(!input.contains(SECRET), "{input}");
        assert!(input.contains(REDACTED_MARKER), "{input}");
        assert!(
            input.contains("\"event_type\":\"MessageReceived\""),
            "{input}"
        );
    }

    #[test]
    fn two_keys_that_mask_alike_both_keep_their_values() {
        let other = "gh-other-secret-9876543210";
        let pending = PendingEvent {
            event_type: "MessageReceived".to_string(),
            payload: json!({ "keyed": { SECRET: 1, other: 2 } }),
        };
        let input = HookInput::masked(&pending, &[SECRET.to_string(), other.to_string()]);
        let keyed = input.payload["keyed"].as_object().unwrap();
        assert_eq!(keyed.len(), 2, "{keyed:?}");
        let mut values: Vec<_> = keyed.values().cloned().collect();
        values.sort_by_key(|v| v.as_i64());
        assert_eq!(values, vec![json!(1), json!(2)]);
        assert!(!input.payload.to_string().contains(SECRET));
        assert!(!input.payload.to_string().contains(other));
    }

    /// The secrets come from the store, so a credential an event quotes never
    /// reaches a hook.
    #[tokio::test]
    async fn the_input_masks_a_stored_credential_an_event_quotes() {
        lucidos_core::core::secret_store::key_store::install_test_key_store();
        let (pool, db) = lucidos_core::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        CredentialStore::upsert(
            &pool,
            &bus,
            "github",
            &[],
            AuthType::Bearer,
            SECRET,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let pending = PendingEvent::of(&note_event(&format!("token {SECRET}")));
        let input = HookInput::read(&pool, &pending).await.unwrap();
        let input = serde_json::to_string(&input).unwrap();
        assert!(!input.contains(SECRET), "{input}");
        assert!(input.contains("\"name\":\"NOTE\""), "{input}");

        lucidos_core::test_support::teardown_test_db(&db).await;
    }
}
