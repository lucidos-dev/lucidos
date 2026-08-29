//! Waking the thread's own agent with what the caller just said.
//!
//! A finished utterance is a message on a chat thread, so it takes the path a
//! typed one takes: `pre_emit_chat_message_received` records it, and
//! `process_message_with_steps` runs the turn. Voice adds no admission rule.
//! Single-flight admission is what turns a second utterance mid-turn into an
//! injection rather than a race.
//!
//! **A seam, so the call loop stays drivable without an engine.** `call.rs`
//! talks to a [`TurnStarter`], the same way it talks to a `CallTransport`. The
//! tests supply one that records, so a whole call runs with no credential.
//!
//! Nothing here reaches back the other way. The reasoner is never told a
//! session is live (ADR 0149): it is shown one, by reading the talker's turns
//! in the thread.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::LucidosEngine;

/// What a spoken turn does to the thread it was spoken on.
#[async_trait]
pub trait TurnStarter: Send + Sync {
    /// The caller finished a thought. Record it, and run the thread's turn.
    ///
    /// Returns once the utterance is recorded, never once the turn is done. The
    /// call loop has audio to pump, and a turn can take minutes.
    async fn heard(&self, thread_id: Uuid, transcript: &str, actor: Option<MessageOrigin>);
}

/// The shipping implementation: the ordinary chat turn, unchanged.
pub struct ThreadTurn {
    engine: Arc<LucidosEngine>,
}

impl ThreadTurn {
    pub fn new(engine: Arc<LucidosEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl TurnStarter for ThreadTurn {
    async fn heard(&self, thread_id: Uuid, transcript: &str, actor: Option<MessageOrigin>) {
        let message = transcript.trim().to_string();
        if message.is_empty() {
            return;
        }

        // The device the call is on, so a spoken message is attributed exactly
        // as a typed one from the same phone.
        let device_id = match &actor {
            Some(MessageOrigin::Device { device_id, .. }) => Some(device_id.clone()),
            _ => None,
        };

        // Record before the turn is queued. At pool-max the turn waits for a
        // slot, and the utterance belongs in the thread either way.
        //
        // This refuses while a question is open, and the turn then answers the
        // question instead of starting a new exchange. That is the behaviour a
        // typed answer already gets, and reading the question aloud is phase 6.
        let pre_emitted = self
            .engine
            .pre_emit_chat_message_received(
                Some(thread_id),
                true,
                ActorMode::Human,
                None,
                &message,
                None,
                device_id.as_deref(),
                None,
                None,
                None,
                actor.clone(),
            )
            .await;

        let engine = self.engine.clone();
        let handle = tokio::spawn(async move {
            // A spoken turn is user-initiated work. So it takes a prioritized
            // slot from the one capacity pool (ADR 0008), exactly as a send from
            // the composer does. Released when this task ends.
            let _user_slot = engine
                .thread_queue
                .acquire_user_slot(
                    Some(thread_id),
                    crate::engine::thread_queue::truncate_summary(&message),
                )
                .await;
            let result = engine
                .process_message_with_steps(
                    &message,
                    None,
                    None,
                    None,
                    None,
                    None,
                    device_id.as_deref(),
                    None,
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    ActorMode::Human,
                    None,
                    None,
                    pre_emitted,
                    None,
                    actor,
                    crate::engine::FollowUpUrgency::Normal,
                )
                .await;
            match result {
                // Drain the orphan chain, as every other spawn site does. An
                // utterance that lands after the loop exits but before cleanup
                // is collected and then dropped, and the caller is never
                // answered. Voice makes that the COMMON case rather than a
                // rare one: talking over a reply is how people talk.
                Ok(res) if !res.orphaned_injections.is_empty() => {
                    crate::api::chat::process_orphan_chain(
                        engine.clone(),
                        thread_id,
                        res.orphaned_injections,
                    )
                    .await;
                }
                Ok(_) => {}
                // No terminator here. The turn settles its own exchange,
                // anchored to its originating event, and an unanchored copy is
                // what the idempotency gate cannot match.
                Err(e) => log!("[Voice] The turn a spoken message started failed: {}", e),
            }
        });
        // The same panic monitoring every other spawn site uses. Without it a
        // panicking turn leaves the thread stuck `running`, with the caller
        // waiting for an answer that is never coming.
        drop(LucidosEngine::monitor_cc_task(
            self.engine.clone(),
            thread_id,
            handle,
        ));
    }
}
