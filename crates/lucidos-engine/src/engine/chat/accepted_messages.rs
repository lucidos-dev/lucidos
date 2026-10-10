//! A message's client event id is accepted once.
//!
//! A client that loses the reply to `POST /api/v1/chat/stream` cannot tell
//! whether its message arrived, so it may post the same body again. The handler
//! answers a repeat with the original ack and starts nothing. The events table
//! cannot answer alone: a coding-agent message is recorded after the ack, so a
//! fast repeat finds no row yet. This record covers that window.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use uuid::Uuid;

/// How long an accepted id is remembered. Long enough for a user to press
/// Retry, and far longer than a coding-agent turn takes to record its message.
const REMEMBERED_FOR: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
pub(crate) struct AcceptedMessages {
    accepted_at: Mutex<HashMap<Uuid, Instant>>,
}

impl AcceptedMessages {
    /// Record `event_id` as accepted. Returns false when it already was, so the
    /// caller answers with the original ack instead of starting a turn.
    pub(crate) fn admit(&self, event_id: Uuid) -> bool {
        self.admit_at(event_id, Instant::now())
    }

    /// Undo an admission whose request then failed before it started anything,
    /// so the user's retry runs.
    pub(crate) fn forget(&self, event_id: Uuid) {
        lock(&self.accepted_at).remove(&event_id);
    }

    fn admit_at(&self, event_id: Uuid, now: Instant) -> bool {
        let mut accepted = lock(&self.accepted_at);
        accepted.retain(|_, at| now.duration_since(*at) < REMEMBERED_FOR);
        if accepted.contains_key(&event_id) {
            return false;
        }
        accepted.insert(event_id, now);
        true
    }
}

/// Is a message with this client event id already recorded? It counts as its
/// own events row, or as a spawn request still in the Thread Queue. A queued
/// request writes no row of its own until a slot frees, however long that is.
pub(crate) async fn chat_event_id_is_recorded(
    pool: &sqlx::PgPool,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE id = $1) \
             OR EXISTS(SELECT 1 FROM thread_queue \
                       WHERE lower(request->>'event_id') = $1::text)",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
}

/// A poisoned lock still holds a usable map: an entry is either present or not.
fn lock(map: &Mutex<HashMap<Uuid, Instant>>) -> MutexGuard<'_, HashMap<Uuid, Instant>> {
    map.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_post_of_an_id_is_admitted_and_a_repeat_is_not() {
        let accepted = AcceptedMessages::default();
        let id = Uuid::new_v4();
        assert!(accepted.admit(id));
        assert!(!accepted.admit(id));
    }

    #[test]
    fn a_forgotten_id_is_admitted_again() {
        let accepted = AcceptedMessages::default();
        let id = Uuid::new_v4();
        assert!(accepted.admit(id));
        accepted.forget(id);
        assert!(accepted.admit(id));
    }

    #[test]
    fn different_ids_do_not_shadow_each_other() {
        let accepted = AcceptedMessages::default();
        assert!(accepted.admit(Uuid::new_v4()));
        assert!(accepted.admit(Uuid::new_v4()));
    }

    #[test]
    fn an_id_is_forgotten_once_its_window_has_passed() {
        let accepted = AcceptedMessages::default();
        let id = Uuid::new_v4();
        let start = Instant::now();
        assert!(accepted.admit_at(id, start));
        assert!(!accepted.admit_at(id, start + REMEMBERED_FOR - Duration::from_secs(1)));
        assert!(accepted.admit_at(id, start + REMEMBERED_FOR));
    }

    /// A message's own row, and a queued spawn request still waiting for a
    /// slot, both count as recorded. An id neither holds does not.
    #[tokio::test]
    async fn a_recorded_or_queued_event_id_is_found_and_a_new_one_is_not() {
        use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
        use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
        use crate::engine::thread_queue::ThreadQueueKind;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let message_id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                text: "hello".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                event_id: Some(message_id),
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("seed MessageReceived")
        .expect("MessageReceived persists");

        let queued_id = Uuid::new_v4();
        bus.emit(BusEvent::System(SystemEvent::ThreadQueued {
            entry_id: Uuid::new_v4(),
            kind: ThreadQueueKind::SubThread,
            trigger_id: None,
            trigger_name: None,
            thread_id: Some(Uuid::new_v4()),
            summary: "queued".into(),
            request: serde_json::json!({ "type": "agent-chat", "event_id": queued_id.to_string() }),
            requeued: false,
            actor: None,
        }))
        .await
        .expect("seed ThreadQueued");

        assert!(chat_event_id_is_recorded(&pool, message_id).await.unwrap());
        assert!(chat_event_id_is_recorded(&pool, queued_id).await.unwrap());
        assert!(!chat_event_id_is_recorded(&pool, Uuid::new_v4())
            .await
            .unwrap());

        teardown_test_db(&db).await;
    }

    #[test]
    fn expired_ids_are_pruned_so_the_record_stays_bounded() {
        let accepted = AcceptedMessages::default();
        let start = Instant::now();
        for _ in 0..100 {
            accepted.admit_at(Uuid::new_v4(), start);
        }
        accepted.admit_at(Uuid::new_v4(), start + REMEMBERED_FOR);
        assert_eq!(lock(&accepted.accepted_at).len(), 1);
    }
}
