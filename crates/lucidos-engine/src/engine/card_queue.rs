use std::sync::Arc;

use tokio::sync::broadcast;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::agent_question::{answer_pending_question, AnswerResult};
use super::event_bus::{BusEvent, CardSlotTaken, EmittedEvent, EventBus};
use super::question_park_sql::{
    card_waits_for_turn_end_sql, ends_question_park, engine_raised_card_sql, open_card_exists,
    turn_moved_on_event_types_sql, waiting_queued_card_sql, PERMISSION_PROMPT_EVENT_TYPES,
};
use super::thread_events::{AnswerKind, EventMeta, ThreadEvent};
use super::LucidosEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

static OLDEST_QUEUED_CARD_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "SELECT q.payload->>'tool_use_id', q.payload, \
           (NOT {engine_raised} AND EXISTS (SELECT 1 FROM events t \
              WHERE t.aggregate_id = q.aggregate_id AND t.sequence > q.sequence \
                AND (t.event_type IN ({moved_on}) OR {own_result}))) \
         FROM events q \
         WHERE q.aggregate_id = $1 AND {waiting} \
           AND NOT ({waits} AND EXISTS (SELECT 1 FROM thread_summaries s \
             WHERE s.thread_id::text = q.aggregate_id AND s.status = {running})) \
         ORDER BY q.sequence LIMIT 1",
        waiting = waiting_queued_card_sql("q"),
        engine_raised = engine_raised_card_sql("q"),
        waits = card_waits_for_turn_end_sql("q"),
        running = super::thread_lifecycle::ThreadStatus::Running.sql_literal(),
        moved_on = turn_moved_on_event_types_sql(),
        own_result = ThreadEvent::question_own_result_sql("q", "t"),
    )
});

pub async fn release(engine: &Arc<LucidosEngine>, thread_id: Uuid) -> Result<(), BoxError> {
    loop {
        if open_card_exists(&mut *engine.pool().acquire().await?, thread_id).await? {
            return Ok(());
        }
        let Some((tool_use_id, payload, turn_moved_on)) =
            sqlx::query_as::<_, (String, serde_json::Value, bool)>(&OLDEST_QUEUED_CARD_SQL)
                .bind(thread_id.to_string())
                .fetch_optional(engine.pool())
                .await?
        else {
            return Ok(());
        };
        if turn_moved_on {
            match answer_pending_question(
                engine,
                thread_id,
                tool_use_id,
                AnswerKind::Superseded,
                None,
            )
            .await
            {
                AnswerResult::Resumed => continue,
                AnswerResult::Conflict(reason) => return Err(reason.into()),
            }
        }
        let meta = EventMeta::from_payload(&payload);
        let event = ThreadEvent::from_stored("UserQuestionQueued", payload)?.queued_into_asked();
        // INVARIANT: The bus drops a release that lost the slot to a concurrent card;
        // that card's close runs this again.
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event,
                meta,
            })
            .await?;
        return Ok(());
    }
}

pub async fn sweep(engine: &Arc<LucidosEngine>) {
    let threads = sqlx::query_scalar::<_, String>(&format!(
        "SELECT DISTINCT q.aggregate_id FROM events q WHERE {}",
        waiting_queued_card_sql("q")
    ))
    .fetch_all(engine.pool())
    .await;
    let threads = match threads {
        Ok(threads) => threads,
        Err(e) => {
            log_legacy!("[CardQueue] Could not list threads with a queued card: {e}");
            return;
        }
    };
    for thread_id in threads.iter().filter_map(|t| Uuid::parse_str(t).ok()) {
        release_or_log(engine, thread_id).await;
    }
}

async fn release_or_log(engine: &Arc<LucidosEngine>, thread_id: Uuid) {
    if let Err(e) = release(engine, thread_id).await {
        log_legacy!("[CardQueue] Could not release a queued card on {thread_id}: {e}");
    }
}

/// INVARIANT: Wait until `thread_id` has no open card. It subscribes before it checks,
/// so a card closed between the check and the wait still wakes it.
pub async fn wait_for_free_slot(bus: &EventBus, thread_id: Uuid) -> Result<(), BoxError> {
    let mut rx = bus.subscribe();
    while open_card_exists(&mut *bus.pool().acquire().await?, thread_id).await? {
        loop {
            match rx.recv().await {
                Ok(emitted) => {
                    if let BusEvent::Thread {
                        thread_id: id,
                        event,
                        ..
                    } = &emitted.typed
                    {
                        if *id == thread_id && emitted.seq.is_some() && may_close_a_card(event) {
                            break;
                        }
                    }
                }
                // INVARIANT: A lag may have hidden the close, so check again.
                Err(broadcast::error::RecvError::Lagged(_)) => break,
                Err(broadcast::error::RecvError::Closed) => {
                    return Err("the event bus closed".into())
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum PromptPost {
    Posted,
    Canceled,
}

/// INVARIANT: Emit an agent permission prompt once the thread's card slot is free.
///
/// Validate refuses the emit with [`CardSlotTaken`] when a concurrent card
/// took the slot first, and this waits again. `cancel` interrupts only the
/// wait, never the emit, so a written card always reaches its caller.
pub async fn post_permission_prompt(
    bus: &EventBus,
    thread_id: Uuid,
    event: ThreadEvent,
    meta: EventMeta,
    cancel: Option<&CancellationToken>,
) -> Result<PromptPost, BoxError> {
    loop {
        let wait = wait_for_free_slot(bus, thread_id);
        match cancel {
            Some(token) => tokio::select! {
                biased;
                _ = token.cancelled() => return Ok(PromptPost::Canceled),
                waited = wait => waited?,
            },
            None => wait.await?,
        }
        match bus
            .emit(BusEvent::Thread {
                thread_id,
                event: event.clone(),
                meta: meta.clone(),
            })
            .await
        {
            Err(e) if e.is::<CardSlotTaken>() => continue,
            emitted => return emitted.map(|_| PromptPost::Posted),
        }
    }
}

/// INVARIANT: Whether a persisted event of this type can close the thread's open card,
/// so a queued card may now post. A `UserQuestionQueued` counts too: the
/// card it queued behind may have closed before this consumer saw it.
pub(crate) fn may_close_a_card(event: &ThreadEvent) -> bool {
    let event_type = event.event_type();
    matches!(
        event_type,
        "UserQuestionAnswered" | "UserQuestionQueued" | "CodingAgentToolResult"
    ) || PERMISSION_PROMPT_EVENT_TYPES
        .iter()
        .any(|(_, resolved)| *resolved == event_type)
        || ends_question_park(event_type)
}

pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(async move {
        if let Err(e) = super::card_retirement::retire_step_cards(&engine).await {
            log_legacy!("[CardQueue] Could not retire the legacy step cards: {e}");
        }
        sweep(&engine).await;
        watch(&engine, rx).await;
    })
}

async fn watch(engine: &Arc<LucidosEngine>, rx: broadcast::Receiver<EmittedEvent>) {
    let stream = BroadcastStream::new(rx);
    tokio::pin!(stream);
    while let Some(result) = stream.next().await {
        let emitted = match result {
            Ok(emitted) => emitted,
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                log_legacy!("[CardQueue] Broadcast lagged by {n} events, sweeping every thread");
                sweep(engine).await;
                continue;
            }
        };
        let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        else {
            continue;
        };
        let Some(seq) = emitted.seq else { continue };
        if let Err(e) =
            super::card_retirement::retire_unwanted_cards(engine, *thread_id, event).await
        {
            log_legacy!(
                "[CardQueue] Could not retire the cards nobody waits for on {thread_id}: {e}"
            );
        }
        if may_close_a_card(event) {
            release_or_log(engine, *thread_id).await;
            if let Err(e) =
                super::card_hold::release_held_wakes(engine, *thread_id, event, seq).await
            {
                log_legacy!(
                    "[CardQueue] Could not hand over the wakes a card held on {thread_id}: {e}"
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "card_queue_tests.rs"]
mod tests;
