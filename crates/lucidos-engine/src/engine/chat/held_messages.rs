//! Held messages: agent-sent messages a coding-agent thread keeps back while it
//! waits on a human. See ADR 0256.
//!
//! The event store is the only state. A `MessageHeld` with no
//! `HeldMessageReleased` naming it is still held, so a hold survives a restart.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::PreEmittedOrigin;
use crate::api::ChatImage;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::LucidosEngine;
use crate::runtime::CodingAgent;

/// The held messages on a thread that nothing has released yet, oldest first.
/// `$1` is the thread id as text.
pub(crate) const UNRELEASED_HELD_MESSAGES_SQL: &str = "\
SELECT e.id, e.payload \
  FROM events e \
 WHERE e.aggregate = 'thread' \
   AND e.aggregate_id = $1 \
   AND e.event_type = 'MessageHeld' \
   AND NOT EXISTS ( \
         SELECT 1 FROM events r \
          WHERE r.aggregate = 'thread' AND r.aggregate_id = $1 \
            AND r.event_type = 'HeldMessageReleased' \
            AND r.payload->>'held_message_id' = e.id::text) \
 ORDER BY e.sequence ASC";

/// One held message, rebuilt for delivery.
#[derive(Debug, Clone)]
pub(crate) struct HeldMessage {
    pub(crate) id: Uuid,
    pub(crate) text: String,
    pub(crate) images: Option<Vec<ChatImage>>,
    pub(crate) origin: Option<MessageOrigin>,
}

/// Every unreleased held message on `thread_id`, oldest first.
pub(crate) async fn unreleased_held_messages(
    pool: &sqlx::PgPool,
    workspace: &Path,
    thread_id: Uuid,
) -> Result<Vec<HeldMessage>, sqlx::Error> {
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(UNRELEASED_HELD_MESSAGES_SQL)
        .bind(thread_id.to_string())
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, payload)| HeldMessage {
            id,
            text: payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            images: super::queued_recovery::images_for_row(workspace, &payload),
            origin: payload
                .get("origin")
                .and_then(|v| serde_json::from_value::<MessageOrigin>(v.clone()).ok()),
        })
        .collect())
}

/// Whether an incoming message on a coding-agent thread must be held.
///
/// Only a fresh agent-sent message is held. A human message answers the
/// question or supersedes it (ADR 0082). A pre-emitted message or an engine
/// re-entry is not a new message (ADR 0255). The hold lasts while the question
/// is answerable or a permission card is pending. It also lasts while older
/// held messages wait, so none is overtaken.
pub(crate) async fn message_is_held(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    mode: ActorMode,
    pre_emitted_origin: Option<PreEmittedOrigin>,
) -> bool {
    if mode != ActorMode::Agent || pre_emitted_origin.is_some() {
        return false;
    }
    if crate::engine::agent_question::lookup_active_question_tool_use_id(pool, thread_id)
        .await
        .is_some()
    {
        return true;
    }
    let waiting =
        match crate::engine::cc_permission::has_pending_permission_card(pool, thread_id).await {
            Ok(true) => Ok(true),
            Ok(false) => has_unreleased_held_messages(pool, thread_id).await,
            Err(e) => Err(e),
        };
    match waiting {
        Ok(held) => held,
        Err(e) => {
            crate::log!(
                "[HeldMessages] hold lookup failed on thread {}: {}; delivering",
                thread_id,
                e
            );
            false
        }
    }
}

async fn has_unreleased_held_messages(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!("SELECT EXISTS ({UNRELEASED_HELD_MESSAGES_SQL})"))
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
}

/// Release the oldest unreleased held message on `thread_id` and return it for
/// delivery, or `None` when nothing is held.
///
/// One at a time, so a crash mid-release strands at most the message it was
/// on. The unique index refuses a release that lost a race; the loop then moves
/// to the next message, so no message is delivered twice.
pub(crate) async fn claim_next_held_message(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    workspace: &Path,
    thread_id: Uuid,
) -> Option<HeldMessage> {
    loop {
        let oldest = match unreleased_held_messages(pool, workspace, thread_id).await {
            Ok(held) => held.into_iter().next()?,
            Err(e) => {
                crate::log!(
                    "[HeldMessages] read failed on thread {}: {}; they stay held",
                    thread_id,
                    e
                );
                return None;
            }
        };
        let released = bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::HeldMessageReleased {
                    held_message_id: oldest.id,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await;
        match released {
            Ok(_) => return Some(oldest),
            Err(e) => crate::log!(
                "[HeldMessages] release of {} on thread {} refused: {}",
                oldest.id,
                thread_id,
                e
            ),
        }
    }
}

/// One lock per thread, so two releases never interleave their deliveries.
/// In memory by design: it guards a delivery in progress, and a restart ends
/// that delivery anyway.
fn delivery_lock(thread_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(thread_id)
        .or_default()
        .clone()
}

/// The event that holds `text` back. Stores its images in the blob store, as
/// `MessageReceived` does, so the release can rebuild them.
pub(crate) fn held_message_event(
    workspace: &Path,
    text: &str,
    images: Option<&[ChatImage]>,
    mode: ActorMode,
    origin: Option<MessageOrigin>,
) -> ThreadEvent {
    ThreadEvent::MessageHeld {
        text: text.to_string(),
        user_image_hashes: super::events::images_to_hashes(workspace, images),
        mode,
        origin,
    }
}

/// How long a release waits for a coding agent to come up after a resume.
const AGENT_START_GRACE: Duration = Duration::from_secs(60);
const AGENT_POLL: Duration = Duration::from_millis(500);

impl LucidosEngine {
    /// Deliver every held message on `thread_id` now, oldest first.
    ///
    /// Boxed for the same reason as `follow_up_child_thread`: delivery re-enters
    /// the message router, which can call back here.
    pub(crate) fn deliver_held_messages(
        self: &Arc<Self>,
        thread_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let lock = delivery_lock(thread_id);
            let _delivering = lock.lock().await;
            while let Some(message) = claim_next_held_message(
                &self.event_bus,
                self.pool(),
                &self.workspace_path,
                thread_id,
            )
            .await
            {
                self.deliver_held_message(thread_id, message).await;
            }
        })
    }

    /// Deliver one released message as the ordinary `MessageReceived` it was
    /// held back from. Emitting it here, rather than in the router, is what
    /// keeps the router from holding it a second time.
    ///
    /// With a live agent the router queues it at once. Without one, the router
    /// would start a session and run its whole turn first. So that turn runs in
    /// the background, and this returns once the session is up. The next
    /// message then queues behind it rather than waiting a turn out.
    async fn deliver_held_message(self: &Arc<Self>, thread_id: Uuid, message: HeldMessage) {
        let received = super::make_message_received(
            &self.workspace_path,
            &message.text,
            message.images.as_deref(),
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            message.origin.clone(),
            None,
        );
        let emitted = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: received,
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await;
        let message_id = match emitted {
            Ok(Some(result)) => result.event_id,
            Ok(None) | Err(_) => {
                crate::log!(
                    "[HeldMessages] could not record released message {} on thread {}: {:?}",
                    message.id,
                    thread_id,
                    emitted.err()
                );
                return;
            }
        };
        let held_id = message.id;
        let agent_is_live = self.agent_is_live(thread_id).await;
        let engine = self.clone();
        let dispatch = async move {
            let delivered = engine
                .process_message_with_steps(
                    &message.text,
                    None,
                    None,
                    None,
                    None,
                    None,
                    message.images.as_deref(),
                    None,
                    Some(true),
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    ActorMode::Agent,
                    None,
                    None,
                    Some(PreEmittedOrigin::Message(message_id)),
                    None,
                    message.origin,
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                    None,
                )
                .await;
            if let Err(e) = delivered {
                crate::log!(
                    "[HeldMessages] delivery of {} on thread {} failed: {}",
                    held_id,
                    thread_id,
                    e
                );
            }
        };
        if agent_is_live {
            dispatch.await;
            return;
        }
        let dispatch: Pin<Box<dyn Future<Output = ()> + Send>> = Box::pin(dispatch);
        tokio::spawn(dispatch);
        let started = Instant::now();
        while !self.agent_is_live(thread_id).await && started.elapsed() < AGENT_START_GRACE {
            tokio::time::sleep(AGENT_POLL).await;
        }
    }

    async fn agent_is_live(&self, thread_id: Uuid) -> bool {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(|s| s.is_live())
    }

    /// Release the held messages once a question is resolved, in the
    /// background, after the agent can take them.
    pub(crate) fn spawn_held_message_release(self: &Arc<Self>, thread_id: Uuid) {
        let engine = self.clone();
        tokio::spawn(async move {
            match has_unreleased_held_messages(engine.pool(), thread_id).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(e) => {
                    crate::log!(
                        "[HeldMessages] backlog lookup failed on thread {}: {}; they stay held",
                        thread_id,
                        e
                    );
                    return;
                }
            }
            engine.wait_until_agent_takes_messages(thread_id).await;
            engine.deliver_held_messages(thread_id).await;
        });
    }

    /// Wait until the thread's coding agent can take a message.
    ///
    /// A resumed session needs time to start, so a missing one gets a grace
    /// period. Claude Code queues a message mid-turn on its own. Codex would
    /// cut the turn short instead, so for Codex this waits for the turn to end.
    async fn wait_until_agent_takes_messages(&self, thread_id: Uuid) {
        let started = Instant::now();
        loop {
            let state = self
                .agent_sessions
                .lock()
                .await
                .get(&thread_id)
                .filter(|s| s.is_live())
                .map(|s| (s.coding_agent, s.is_in_flight()));
            match state {
                Some((CodingAgent::ClaudeCode, _)) | Some((CodingAgent::Codex, false)) => return,
                Some((CodingAgent::Codex, true)) => {}
                None if started.elapsed() >= AGENT_START_GRACE => return,
                None => {}
            }
            tokio::time::sleep(AGENT_POLL).await;
        }
    }
}

/// Whether resolving a question with `answer` releases the held messages.
/// Cancel means stop, so it keeps them held until a human writes again.
pub(crate) fn answer_releases_held_messages(answer: &AnswerKind) -> bool {
    !matches!(answer, AnswerKind::Canceled)
}

#[cfg(test)]
#[path = "held_messages_tests.rs"]
mod tests;
