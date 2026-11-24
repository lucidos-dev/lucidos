//! Durable next-turn rung picks and fixed live-turn settings (ADR 0544).

use uuid::Uuid;

use crate::core::ResolvedModelSelection;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::turn_start_gate::TurnStartGate;

/// What such a refusal says to the person.
pub(crate) const TURN_RUNNING_MESSAGE: &str =
    "The model and effort stay fixed while a turn runs. Pick again when it ends.";

pub(crate) fn turn_open(status: ThreadStatus) -> bool {
    matches!(
        status,
        ThreadStatus::Running | ThreadStatus::WaitingForUserAnswer | ThreadStatus::Paused
    )
}

/// The thread's projected status. A thread with no row reads as idle.
async fn thread_status(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<ThreadStatus, sqlx::Error> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    Ok(ThreadStatus::parse(status.as_deref().unwrap_or_default()))
}

/// What [`LucidosEngine::record_chat_settings`] did with a pick.
///
/// [`LucidosEngine::record_chat_settings`]: crate::engine::LucidosEngine::record_chat_settings
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatSettingsOutcome {
    /// Recorded. The thread's next turn runs on it.
    Recorded,
}

/// Record a resolved next-turn pick, ordered against turn admission.
async fn record_pick(
    bus: &EventBus,
    gate: &TurnStartGate,
    thread_id: Uuid,
    pair: impl std::future::Future<Output = ResolvedModelSelection>,
    picked_model: String,
    actor: Option<MessageOrigin>,
) -> Result<ChatSettingsOutcome, Box<dyn std::error::Error + Send + Sync>> {
    let _pick = gate.picks.lock().await;
    let _hold = gate.hold(thread_id).await;
    let ResolvedModelSelection {
        model,
        reasoning_effort,
        provider,
    } = pair.await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChatSettingsChanged {
            model: model.unwrap_or(picked_model),
            reasoning_effort,
            provider,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::with_actor(actor)
        },
    })
    .await?;
    Ok(ChatSettingsOutcome::Recorded)
}

/// Applied turn stamps survive reload and continuations. Older turns fall
/// back to their latest non-injected starter.
async fn live_pair(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<ResolvedModelSelection, sqlx::Error> {
    let row = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
        "SELECT payload->>'model', payload->>'reasoning_effort', payload->>'provider' FROM events
         WHERE thread_id = $1 AND (
           (event_type = 'ChatTurnSettingsResolved' AND payload->>'request_event_id' IS NOT NULL)
           OR (event_type IN ('MessageReceived', 'TriggerStarted') AND payload->>'request_event_id' IS NULL)
         ) ORDER BY (event_type = 'ChatTurnSettingsResolved') DESC, sequence DESC LIMIT 1"
    ).bind(thread_id).fetch_optional(pool).await?;
    Ok(row
        .map(
            |(model, reasoning_effort, provider)| ResolvedModelSelection {
                model,
                reasoning_effort,
                provider,
            },
        )
        .unwrap_or_default())
}

async fn turn_runs(
    pool: &sqlx::PgPool,
    gate: &TurnStartGate,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let _pick = gate.picks.lock().await;
    let Some(_hold) = gate.try_hold(thread_id) else {
        return Ok(true);
    };
    Ok(turn_open(thread_status(pool, thread_id).await?))
}

async fn running_turn_overrides(
    pool: &sqlx::PgPool,
    gate: &TurnStartGate,
    thread_id: Uuid,
) -> Result<ResolvedModelSelection, sqlx::Error> {
    if turn_runs(pool, gate, thread_id).await? {
        live_pair(pool, thread_id).await
    } else {
        Ok(ResolvedModelSelection::default())
    }
}

#[easy_ext::ext(RungExt)]
pub(crate) impl crate::engine::LucidosEngine {
    /// Whether a turn runs or is starting on a Lucidos Agent thread.
    async fn chat_turn_runs(&self, thread_id: Uuid) -> Result<bool, sqlx::Error> {
        turn_runs(self.pool(), &self.turn_start_gate, thread_id).await
    }

    /// See [`running_turn_overrides`].
    async fn running_turn_overrides(
        &self,
        thread_id: Uuid,
    ) -> Result<ResolvedModelSelection, sqlx::Error> {
        running_turn_overrides(self.pool(), &self.turn_start_gate, thread_id).await
    }

    /// Record a person's rung pick on a Lucidos Agent thread. It records the
    /// pair a turn resolves from the pick, so the stamp names what runs next.
    async fn record_chat_settings(
        &self,
        thread_id: Uuid,
        pick: ResolvedModelSelection,
        actor: Option<MessageOrigin>,
    ) -> Result<ChatSettingsOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let picked_model = pick.model.clone().unwrap_or_default();
        record_pick(
            &self.event_bus,
            &self.turn_start_gate,
            thread_id,
            self.resolve_chat_pair(thread_id, pick),
            picked_model,
            actor,
        )
        .await
    }

    /// The pair a turn on this thread answers with, given what a caller asked
    /// for. Resolved exactly as the turn resolves it.
    async fn resolve_chat_pair(
        &self,
        thread_id: Uuid,
        overrides: ResolvedModelSelection,
    ) -> ResolvedModelSelection {
        crate::engine::chat::process::resolve_available_route_overrides(
            self.pool(),
            &self.model_routes(),
            None,
            Some(thread_id),
            None,
            overrides,
        )
        .await
    }

    /// Read the applied pair, excluding pending picks and injected messages.
    async fn live_chat_pair(&self, thread_id: Uuid) -> Result<ResolvedModelSelection, sqlx::Error> {
        live_pair(self.pool(), thread_id).await
    }

    /// Whether a message for a running turn names a pair other than the one
    /// that turn answers with. The caller refuses such a message before it
    /// records anything, so its override is never dropped silently.
    async fn mid_turn_override_differs(
        &self,
        thread_id: Uuid,
        overrides: ResolvedModelSelection,
    ) -> Result<bool, sqlx::Error> {
        if overrides == ResolvedModelSelection::default() || !self.chat_turn_runs(thread_id).await?
        {
            return Ok(false);
        }
        let live = self.live_chat_pair(thread_id).await?;
        Ok(self.resolve_chat_pair(thread_id, overrides).await != live)
    }
}

#[cfg(test)]
#[path = "rung_tests.rs"]
mod tests;
