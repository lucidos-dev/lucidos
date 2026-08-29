//! Settle the calls the last engine died holding.
//!
//! A voice session cannot outlive the process holding its socket, so every
//! `VoiceSessionStarted` still unpaired at boot belongs to a call that is over.
//! The engine that held it never got to say so.
//!
//! This is what keeps the pair countable. Without it a killed engine leaves a
//! start nothing ever answers, and "how many calls did I make" stops having an
//! answer.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent, VoiceSessionEndReason};

/// Pair every start the last process left open.
///
/// `duration_secs` is zero, because the engine that held the clock is gone. A
/// number derived from event timestamps would be the age of the row rather
/// than the length of the call.
pub async fn settle_orphan_voice_sessions(pool: &PgPool, event_bus: &EventBus) {
    let rows: Vec<(Uuid, Uuid)> = match sqlx::query_as(
        "SELECT e.thread_id, (e.payload->>'session_id')::uuid \
         FROM events e \
         WHERE e.event_type = 'VoiceSessionStarted' \
           AND e.thread_id IS NOT NULL \
           AND NOT EXISTS ( \
             SELECT 1 FROM events x \
             WHERE x.event_type = 'VoiceSessionEnded' \
               AND x.payload->>'session_id' = e.payload->>'session_id' \
           )",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            log!("[Recovery] The orphan voice-session query failed: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }
    log!(
        "[Recovery] Settling {} voice session(s) the last engine died holding",
        rows.len()
    );

    for (thread_id, session_id) in rows {
        event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::VoiceSessionEnded {
                        session_id,
                        reason: VoiceSessionEndReason::EngineShutdown,
                        duration_secs: 0,
                    },
                    meta: EventMeta::with_actor(Some(MessageOrigin::system())),
                },
                "[Recovery] VoiceSessionEnded (orphan)",
            )
            .await;
    }
}
