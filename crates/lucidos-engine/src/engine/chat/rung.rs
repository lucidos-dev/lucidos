//! The Lucidos Agent's *rung lock*. A thread's model and effort stay fixed
//! while a turn runs. A pick between turns is recorded as a
//! `ChatSettingsChanged` (ADR 0544). ADR 0542 does the same for coding agents.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::core::ResolvedModelSelection;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_lifecycle::ThreadStatus;

/// The `reason` slug of a pick or message refused because a turn runs. A
/// coding agent's refused pick carries the same slug.
pub(crate) const TURN_RUNNING_REASON: &str = "turn-running";

/// What such a refusal says to the person.
pub(crate) const TURN_RUNNING_MESSAGE: &str =
    "The model and effort stay fixed while a turn runs. Pick again when it ends.";

/// Orders a turn's start with a rung pick, per thread. A turn holds its
/// thread's gate from resolving its pair until its starter event is recorded.
/// A pick takes the same gate before it reads the status. So a turn either
/// resolves after the pick is recorded, or the pick sees the turn and refuses.
#[derive(Default)]
pub(crate) struct TurnStartGate {
    gates: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    /// Picks queue here before they try the gate, so a busy gate always
    /// means a turn is starting, never another pick.
    picks: tokio::sync::Mutex<()>,
}

/// One holder of a thread's [`TurnStartGate`]. Dropping it frees the gate,
/// and forgets the thread's entry once nobody else holds or waits on it.
pub(crate) struct TurnStartHold<'a> {
    gate: &'a TurnStartGate,
    thread_id: Uuid,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl TurnStartGate {
    fn entry(&self, thread_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        self.gates
            .lock()
            .unwrap()
            .entry(thread_id)
            .or_default()
            .clone()
    }

    /// Wait for the thread's gate, then hold it.
    pub(crate) async fn hold(&self, thread_id: Uuid) -> TurnStartHold<'_> {
        let guard = self.entry(thread_id).lock_owned().await;
        TurnStartHold {
            gate: self,
            thread_id,
            guard: Some(guard),
        }
    }

    /// Hold the thread's gate, or `None` while a turn is starting there.
    pub(crate) fn try_hold(&self, thread_id: Uuid) -> Option<TurnStartHold<'_>> {
        let hold = TurnStartHold {
            gate: self,
            thread_id,
            guard: self.entry(thread_id).try_lock_owned().ok(),
        };
        hold.guard.is_some().then_some(hold)
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.gates.lock().unwrap().len()
    }
}

impl Drop for TurnStartHold<'_> {
    fn drop(&mut self) {
        let mut gates = self.gate.gates.lock().unwrap();
        drop(self.guard.take());
        // Only the map's own reference is left, so nobody holds or waits.
        // Waiters clone the entry under this map lock, so none appears now.
        if gates
            .get(&self.thread_id)
            .is_some_and(|gate| Arc::strong_count(gate) == 1)
        {
            gates.remove(&self.thread_id);
        }
    }
}

/// Whether a status means a turn runs on the thread: it works, waits on a
/// question, or is paused. The same three statuses lock a coding agent's rung.
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
    /// Refused: a turn runs or is starting, and its pair stays fixed.
    TurnRunning,
}

/// Record a pick as a `ChatSettingsChanged`, unless a turn runs or is
/// starting. `pair` names what to record. It resolves under the gate, so no
/// turn starts between the check and the emit.
async fn record_pick(
    bus: &EventBus,
    gate: &TurnStartGate,
    thread_id: Uuid,
    pair: impl std::future::Future<Output = ResolvedModelSelection>,
    picked_model: String,
    actor: Option<MessageOrigin>,
) -> Result<ChatSettingsOutcome, Box<dyn std::error::Error + Send + Sync>> {
    let _pick = gate.picks.lock().await;
    let Some(_hold) = gate.try_hold(thread_id) else {
        return Ok(ChatSettingsOutcome::TurnRunning);
    };
    if turn_open(thread_status(bus.pool(), thread_id).await?) {
        return Ok(ChatSettingsOutcome::TurnRunning);
    }
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

impl crate::engine::LucidosEngine {
    /// Whether a turn runs or is starting on a Lucidos Agent thread, so its
    /// rung is locked.
    pub(crate) async fn chat_turn_runs(&self, thread_id: Uuid) -> Result<bool, sqlx::Error> {
        let _pick = self.turn_start_gate.picks.lock().await;
        let Some(_hold) = self.turn_start_gate.try_hold(thread_id) else {
            return Ok(true);
        };
        Ok(turn_open(thread_status(self.pool(), thread_id).await?))
    }

    /// Record a person's rung pick on a Lucidos Agent thread. It records the
    /// pair a turn resolves from the pick, so the stamp names what runs next.
    pub async fn record_chat_settings(
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
    pub(crate) async fn resolve_chat_pair(
        &self,
        thread_id: Uuid,
        overrides: ResolvedModelSelection,
    ) -> ResolvedModelSelection {
        let router = self.current_provider();
        let configured = router.configured_providers();
        crate::engine::chat::process::resolve_route_overrides(
            self.pool(),
            crate::core::ModelDefaults {
                registry: self.model_registry(),
                fallback_model: router.default_model(),
            },
            |kind| configured.as_ref().is_none_or(|set| set.contains(&kind)),
            None,
            Some(thread_id),
            None,
            overrides,
        )
        .await
    }

    /// Whether a message for a running turn names a pair other than the one
    /// that turn answers with. The caller refuses such a message before it
    /// records anything, so its override is never dropped silently.
    pub(crate) async fn mid_turn_override_differs(
        &self,
        thread_id: Uuid,
        overrides: ResolvedModelSelection,
    ) -> Result<bool, sqlx::Error> {
        if overrides == ResolvedModelSelection::default() || !self.chat_turn_runs(thread_id).await?
        {
            return Ok(false);
        }
        let live = self
            .resolve_chat_pair(thread_id, ResolvedModelSelection::default())
            .await;
        Ok(self.resolve_chat_pair(thread_id, overrides).await != live)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_chat_thread, setup_test_db, teardown_test_db};

    fn pair(model: &str, effort: &str) -> ResolvedModelSelection {
        ResolvedModelSelection {
            model: Some(model.to_string()),
            reasoning_effort: Some(effort.to_string()),
            provider: None,
        }
    }

    async fn set_status(pool: &sqlx::PgPool, thread_id: Uuid, status: &str) {
        sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
            .bind(thread_id)
            .bind(status)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn picks_on(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<serde_json::Value> {
        sqlx::query_scalar(
            "SELECT payload FROM events \
             WHERE thread_id = $1 AND event_type = 'ChatSettingsChanged' ORDER BY sequence",
        )
        .bind(thread_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn pick(bus: &EventBus, gate: &TurnStartGate, thread_id: Uuid) -> ChatSettingsOutcome {
        record_pick(
            bus,
            gate,
            thread_id,
            std::future::ready(pair("model-b", "high")),
            "model-b".to_string(),
            None,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn an_idle_thread_records_the_resolved_pick() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let gate = TurnStartGate::default();
        let thread = seed_chat_thread(&bus, "hello").await;
        set_status(&pool, thread, "idle").await;
        assert_eq!(
            pick(&bus, &gate, thread).await,
            ChatSettingsOutcome::Recorded
        );
        let picks = picks_on(&pool, thread).await;
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0]["model"], "model-b");
        assert_eq!(picks[0]["reasoning_effort"], "high");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn every_open_turn_status_refuses_the_pick() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let gate = TurnStartGate::default();
        let thread = seed_chat_thread(&bus, "hello").await;
        for status in ["running", "waiting_for_user_answer", "paused"] {
            set_status(&pool, thread, status).await;
            assert_eq!(
                pick(&bus, &gate, thread).await,
                ChatSettingsOutcome::TurnRunning,
                "{status}"
            );
        }
        assert!(picks_on(&pool, thread).await.is_empty());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_starting_turn_refuses_the_pick() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let gate = TurnStartGate::default();
        let thread = seed_chat_thread(&bus, "hello").await;
        set_status(&pool, thread, "idle").await;
        let turn = gate.hold(thread).await;
        assert_eq!(
            pick(&bus, &gate, thread).await,
            ChatSettingsOutcome::TurnRunning
        );
        drop(turn);
        assert!(picks_on(&pool, thread).await.is_empty());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Two quick picks on an idle thread both land. Neither reads the other
    /// as a starting turn.
    #[tokio::test]
    async fn concurrent_picks_on_an_idle_thread_are_both_recorded() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let gate = TurnStartGate::default();
        let thread = seed_chat_thread(&bus, "hello").await;
        set_status(&pool, thread, "idle").await;
        let (first, second) = tokio::join!(pick(&bus, &gate, thread), pick(&bus, &gate, thread));
        assert_eq!(first, ChatSettingsOutcome::Recorded);
        assert_eq!(second, ChatSettingsOutcome::Recorded);
        assert_eq!(picks_on(&pool, thread).await.len(), 2);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A turn that waits on the gate while a pick is recorded resolves after
    /// it, so it reads the pick.
    #[tokio::test]
    async fn a_turn_waiting_on_the_gate_starts_after_the_pick() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let gate = std::sync::Arc::new(TurnStartGate::default());
        let thread = seed_chat_thread(&bus, "hello").await;
        set_status(&pool, thread, "idle").await;
        let (resolving_tx, resolving_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let resolve = async move {
            resolving_tx.send(()).unwrap();
            release_rx.await.unwrap();
            pair("model-b", "high")
        };
        let picking = {
            let (bus, gate) = (bus.clone(), gate.clone());
            tokio::spawn(async move {
                record_pick(&bus, &gate, thread, resolve, "model-b".into(), None).await
            })
        };
        resolving_rx.await.unwrap();
        let turn = {
            let (pool, gate) = (pool.clone(), gate.clone());
            tokio::spawn(async move {
                let _start = gate.hold(thread).await;
                crate::core::PreferenceStore::last_thread_chat_settings(&pool, thread, None, None)
                    .await
            })
        };
        release_tx.send(()).unwrap();
        assert_eq!(
            picking.await.unwrap().unwrap(),
            ChatSettingsOutcome::Recorded
        );
        assert_eq!(turn.await.unwrap().model.as_deref(), Some("model-b"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_pick_cannot_take_the_gate_while_a_turn_starts() {
        let gate = TurnStartGate::default();
        let thread = Uuid::new_v4();
        let turn = gate.hold(thread).await;
        assert!(gate.try_hold(thread).is_none());
        assert!(gate.try_hold(Uuid::new_v4()).is_some());
        drop(turn);
        assert!(gate.try_hold(thread).is_some());
    }

    #[tokio::test]
    async fn a_freed_gate_forgets_its_thread() {
        let gate = TurnStartGate::default();
        let thread = Uuid::new_v4();
        drop(gate.hold(thread).await);
        assert!(gate.try_hold(thread).is_some());
        assert_eq!(gate.tracked(), 0);
    }
}
