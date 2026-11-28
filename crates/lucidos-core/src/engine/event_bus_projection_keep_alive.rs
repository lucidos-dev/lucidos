use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use super::super::EventBus;
use crate::engine::thread_events::{
    EngineReason, MessageOrigin, ThreadEvent, KEEP_ALIVE_ENDING_EVENT_TYPES,
};

pub(crate) type KeepAliveTurns = Arc<Mutex<HashSet<Uuid>>>;

const SILENCED_DURING_KEEP_ALIVE: &[&str] = &[
    "CodingAgentTextStreamed",
    "CodingAgentThoughtStreamed",
    "CodingAgentToolCalled",
    "CodingAgentToolResult",
    "CodingAgentPromptSent",
    "ResponseGenerated",
    "ResponseFailed",
    "ResponseCanceled",
    "ResponseAborted",
    "SessionEnded",
    "CodingAgentIdled",
    "ThreadReadRequested",
    "ThreadReadNotRequested",
];

pub(crate) fn is_keep_alive_anchor(event: &ThreadEvent) -> bool {
    matches!(
        event,
        ThreadEvent::PromptInjected {
            origin: Some(MessageOrigin::Engine {
                reason: EngineReason::CacheKeepAlive
            }),
            ..
        }
    )
}

pub struct KeepAliveTurn {
    bus: EventBus,
    thread_id: Uuid,
}

impl Drop for KeepAliveTurn {
    fn drop(&mut self) {
        if self
            .bus
            .keep_alive_turns
            .lock()
            .unwrap()
            .contains(&self.thread_id)
        {
            let (bus, thread_id) = (self.bus.clone(), self.thread_id);
            tokio::spawn(async move { bus.mark_keep_alive_turn(thread_id, false).await });
        }
    }
}

impl EventBus {
    pub async fn begin_keep_alive_turn(&self, thread_id: Uuid) -> KeepAliveTurn {
        self.mark_keep_alive_turn(thread_id, true).await;
        KeepAliveTurn {
            bus: self.clone(),
            thread_id,
        }
    }

    pub fn in_keep_alive_turn(&self, thread_id: Uuid) -> bool {
        self.keep_alive_turns.lock().unwrap().contains(&thread_id)
    }

    /// INVARIANT: Mark or clear a thread's keep-alive turn. Only the run that holds the
    /// thread for a keep-alive calls this. A failed write is logged: the set
    /// still silences the turn, and the boot reset clears a stale column.
    pub async fn mark_keep_alive_turn(&self, thread_id: Uuid, on: bool) {
        {
            let mut turns = self.keep_alive_turns.lock().unwrap();
            if on {
                turns.insert(thread_id);
            } else {
                turns.remove(&thread_id);
            }
        }
        if let Err(e) =
            sqlx::query("UPDATE thread_summaries SET keep_alive_turn = $2 WHERE thread_id = $1")
                .bind(thread_id)
                .bind(on)
                .execute(self.pool())
                .await
        {
            crate::log_legacy!(
                "[KeepAlive] Could not set keep_alive_turn={on} on thread {thread_id}: {e}"
            );
        }
    }

    /// INVARIANT: Clear every keep-alive flag at boot. No session survives a restart, so
    /// a set flag names a keep-alive the stop cut off. Run it after recovery,
    /// which reads the flag to leave such a turn alone, and before the first
    /// beat.
    pub async fn reset_keep_alive_turns(&self) {
        match sqlx::query(
            "UPDATE thread_summaries SET keep_alive_turn = FALSE WHERE keep_alive_turn",
        )
        .execute(self.pool())
        .await
        {
            Ok(done) if done.rows_affected() > 0 => crate::log_legacy!(
                "[KeepAlive] Cleared {} keep-alive turn(s) a restart cut off",
                done.rows_affected()
            ),
            Ok(_) => {}
            Err(e) => {
                crate::log_legacy!("[KeepAlive] Could not clear the keep-alive flags at boot: {e}")
            }
        }
    }

    /// INVARIANT: Whether the projection must skip `event`. A card raised during a
    /// keep-alive ends it here, in the same transaction that projects the card.
    pub(super) async fn skips_for_keep_alive(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
        event: &ThreadEvent,
    ) -> Result<bool, sqlx::Error> {
        if is_keep_alive_anchor(event) {
            return Ok(true);
        }
        if !self.in_keep_alive_turn(thread_id) {
            return Ok(false);
        }
        let event_type = event.event_type();
        if KEEP_ALIVE_ENDING_EVENT_TYPES.contains(&event_type) {
            self.keep_alive_turns.lock().unwrap().remove(&thread_id);
            sqlx::query("UPDATE thread_summaries SET keep_alive_turn = FALSE WHERE thread_id = $1")
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
            return Ok(false);
        }
        Ok(SILENCED_DURING_KEEP_ALIVE.contains(&event_type))
    }
}

#[cfg(test)]
#[path = "event_bus_projection_keep_alive_tests.rs"]
mod tests;
