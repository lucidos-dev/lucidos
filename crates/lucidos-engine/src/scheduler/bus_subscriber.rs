use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::task_runner::{
    fire_matching_triggers, handle_trigger_event, rearm_after_lag, TrackedTask,
};
use super::TriggerDispatch;
use super::{load_trigger_groups, stored_trigger_dispatch, StoredTriggerCarrier};
use super::{reload_backup_schedule, trigger_dispatch, BackupArm, BackupRunner};
use crate::core::{event_subscription::EventSubscription, prefs};
use crate::engine::event_bus::{
    BusEvent, EmittedEvent, EventBus, LagReplay, ReplayWindow, SystemEvent,
};
use crate::engine::thread_queue::ThreadQueueKind;
use crate::engine::types::SharedEngine;
use crate::triggers::{TriggerConfig, TriggerGroupEventRow, TriggerGroupProjection};

const REPLAY_PAGE: i64 = 500;

pub(super) type TriggerConfigs = Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>;

pub(super) type TriggerGroups = Arc<std::sync::RwLock<TriggerGroupProjection>>;

#[async_trait]
pub(super) trait SchedulerEffects: Send + Sync {
    async fn apply_trigger_lifecycle(&self, event_type: &str, trigger_id: &str);
    async fn rearm_cron_after_lag(&self);
    async fn reload_backup_schedule(&self);
    async fn dispatch_trigger(
        &self,
        dispatch: &TriggerDispatch,
        source_event_id: Uuid,
        excluded: &HashSet<String>,
    );
}

pub(super) struct EngineEffects {
    pub(super) engine: SharedEngine,
    pub(super) pool: PgPool,
    pub(super) trigger_configs: TriggerConfigs,
    pub(super) tracked_tasks: Arc<RwLock<HashMap<Uuid, TrackedTask>>>,
    pub(super) backup_runner: BackupRunner,
}

#[async_trait]
impl SchedulerEffects for EngineEffects {
    async fn apply_trigger_lifecycle(&self, event_type: &str, trigger_id: &str) {
        handle_trigger_event(
            event_type,
            trigger_id,
            &self.trigger_configs,
            &self.tracked_tasks,
            &self.engine,
        )
        .await;
    }

    async fn rearm_cron_after_lag(&self) {
        rearm_after_lag(&self.trigger_configs, &self.tracked_tasks, &self.engine).await;
    }

    async fn reload_backup_schedule(&self) {
        reload_backup_schedule(
            &self.backup_runner,
            &self.engine,
            &self.pool,
            BackupArm::Reload,
        )
        .await;
    }

    async fn dispatch_trigger(
        &self,
        dispatch: &TriggerDispatch,
        source_event_id: Uuid,
        excluded: &HashSet<String>,
    ) {
        fire_matching_triggers(
            dispatch,
            source_event_id,
            excluded,
            &self.trigger_configs,
            &self.engine,
        )
        .await;
    }
}

pub(super) struct BusSubscriber<E> {
    effects: E,
    bus: EventBus,
    trigger_configs: TriggerConfigs,
    trigger_groups: TriggerGroups,
    lag_replay: LagReplay,
}

impl<E: SchedulerEffects> BusSubscriber<E> {
    /// INVARIANT: Build the subscriber around the position its receiver opened with.
    pub(super) fn new(
        effects: E,
        bus: EventBus,
        trigger_configs: TriggerConfigs,
        trigger_groups: TriggerGroups,
        lag_replay: LagReplay,
    ) -> Self {
        Self {
            effects,
            bus,
            trigger_configs,
            trigger_groups,
            lag_replay,
        }
    }

    pub(super) async fn step(
        &mut self,
        received: Result<EmittedEvent, RecvError>,
    ) -> ControlFlow<()> {
        match received {
            Ok(emitted) => {
                self.on_frame(&emitted).await;
                ControlFlow::Continue(())
            }
            Err(RecvError::Lagged(n)) => {
                crate::log_legacy!("[Scheduler] EventBus subscriber lagged by {} events", n);
                self.effects.rearm_cron_after_lag().await;
                self.effects.reload_backup_schedule().await;
                self.refresh_trigger_groups().await;
                self.replay_missed_fires().await;
                ControlFlow::Continue(())
            }
            Err(RecvError::Closed) => {
                crate::log_legacy!("[Scheduler] EventBus closed, stopping trigger subscriber");
                ControlFlow::Break(())
            }
        }
    }

    async fn on_frame(&mut self, emitted: &EmittedEvent) {
        let first_sight = self.lag_replay.live(emitted);
        if let BusEvent::System(se) = &emitted.typed {
            match se {
                SystemEvent::TriggerCreated { trigger_id, .. }
                | SystemEvent::TriggerUpdated { trigger_id, .. }
                | SystemEvent::TriggerDeleted { trigger_id, .. }
                | SystemEvent::TriggerEnabled { trigger_id, .. }
                | SystemEvent::TriggerDisabled { trigger_id, .. }
                | SystemEvent::TriggerExecuted { trigger_id, .. } => {
                    self.effects
                        .apply_trigger_lifecycle(se.event_type(), trigger_id)
                        .await;
                }
                SystemEvent::TriggerGroupCreated { payload, .. }
                | SystemEvent::TriggerGroupRenamed { payload, .. }
                | SystemEvent::TriggerGroupReordered { payload, .. }
                | SystemEvent::TriggerGroupDeleted { payload, .. } => {
                    if let Some(seq) = emitted.seq {
                        self.trigger_groups
                            .write()
                            .unwrap()
                            .apply(&TriggerGroupEventRow {
                                seq,
                                event_type: se.event_type().to_owned(),
                                payload: payload.clone(),
                                created: emitted.created,
                            });
                    }
                }
                SystemEvent::TimezoneSet { .. } => {
                    self.effects.reload_backup_schedule().await;
                }
                // INVARIANT: Re-arm when the backup schedule or provider changes via any
                // write path: the agent's `set_preference` (the tool layer
                // cannot reach the scheduler) or the HTTP handler. Idempotent
                // with the handler's own direct call.
                SystemEvent::PreferencesChanged { key, .. }
                    if key == prefs::BACKUP_SCHEDULE.key()
                        || key == prefs::BACKUP_PROVIDER.key() =>
                {
                    self.effects.reload_backup_schedule().await;
                }
                _ => {}
            }
        }
        // INVARIANT: A replay already offered this one, and it arrived late from the
        // buffer. The state arms above are idempotent, so they ran anyway.
        if !first_sight {
            return;
        }
        if let Some(dispatch) = trigger_dispatch(emitted) {
            self.effects
                .dispatch_trigger(&dispatch, emitted.event_id, &HashSet::new())
                .await;
        }
    }

    async fn refresh_trigger_groups(&self) {
        match load_trigger_groups(self.bus.pool()).await {
            Ok(groups) => self.trigger_groups.write().unwrap().merge(groups),
            Err(e) => crate::log_legacy!(
                "[Scheduler] Lag repair could not re-read trigger groups, keeping them: {}",
                e
            ),
        }
    }

    async fn replay_missed_fires(&mut self) {
        let Some(window) = self.lag_replay.window(self.bus.pool()).await else {
            return;
        };
        let mut offered_through = window.after;
        match self.replay_window(&window, &mut offered_through).await {
            Ok(offered) => {
                self.lag_replay.covered(&window);
                crate::log_legacy!(
                    "[Scheduler] Lag repair offered {} missed event(s) to the trigger matcher",
                    offered
                );
            }
            Err(e) => {
                self.lag_replay.covered(&window.up_to(offered_through));
                crate::log_legacy!(
                    "[Scheduler] Lag repair could not read the missed events after {}, so \
                     the next lag replays them: {}",
                    offered_through,
                    e
                );
            }
        }
    }

    async fn replay_window(
        &self,
        window: &ReplayWindow,
        offered_through: &mut i64,
    ) -> Result<usize, sqlx::Error> {
        let types = subscribed_event_types(&self.trigger_configs);
        if types.is_empty() {
            *offered_through = window.through;
            return Ok(0);
        }
        let pool = self.bus.pool();
        let exclusions = ReplayExclusions::read(pool, window.after, &self.trigger_configs).await?;
        let mut after = window.after;
        let mut offered = 0;
        loop {
            let rows: Vec<StoredTriggerCarrier> = sqlx::query_as(&format!(
                "SELECT id, sequence, event_type, payload, thread_id, \
                        trigger_chain_depth, emitting_trigger_id \
                 FROM events \
                 WHERE sequence > $1 AND sequence <= $2 AND event_type = ANY($3) AND {} \
                 ORDER BY sequence LIMIT $4",
                crate::engine::thread_events::not_an_engine_turn_idle_sql("")
            ))
            .bind(after)
            .bind(window.through)
            .bind(&types)
            .bind(REPLAY_PAGE)
            .fetch_all(pool)
            .await?;
            let Some(last) = rows.last() else {
                *offered_through = window.through;
                return Ok(offered);
            };
            after = last.sequence;
            for row in rows.iter().filter(|row| window.includes(row.sequence)) {
                if let Some(dispatch) = stored_trigger_dispatch(row) {
                    let excluded = exclusions.for_row(row, &self.trigger_configs);
                    self.effects
                        .dispatch_trigger(&dispatch, row.id, &excluded)
                        .await;
                    offered += 1;
                }
            }
            *offered_through = after;
            if (rows.len() as i64) < REPLAY_PAGE {
                *offered_through = window.through;
                return Ok(offered);
            }
        }
    }
}

fn subscribed_event_types(trigger_configs: &TriggerConfigs) -> Vec<String> {
    let configs = trigger_configs.read().unwrap();
    let types: HashSet<&str> = configs
        .values()
        .filter(|t| !t.paused)
        .flat_map(|t| &t.on)
        .flat_map(EventSubscription::event_type_candidates)
        .collect();
    types.into_iter().map(str::to_string).collect()
}

#[derive(Debug, Default)]
struct ReplayExclusions {
    /// INVARIANT: Per source event, the triggers its live dispatch already fired. Every
    /// event-trigger submit records a `ThreadQueued` row before it returns.
    fired: HashMap<Uuid, HashSet<String>>,
    created: HashMap<String, i64>,
    known: HashSet<String>,
}

impl ReplayExclusions {
    async fn read(
        pool: &PgPool,
        after: i64,
        trigger_configs: &TriggerConfigs,
    ) -> Result<Self, sqlx::Error> {
        let known = trigger_configs.read().unwrap().keys().cloned().collect();
        let fired: Vec<(String, Uuid)> = sqlx::query_as(
            "SELECT payload->'data'->'request'->>'trigger_id', \
                    (payload->'data'->'request'->>'source_event_id')::uuid \
             FROM events \
             WHERE event_type = 'ThreadQueued' AND sequence > $1 \
               AND payload->'data'->>'kind' = $2 \
               AND payload->'data'->'request' ? 'source_event_id'",
        )
        .bind(after)
        .bind(ThreadQueueKind::EventTrigger.as_str())
        .fetch_all(pool)
        .await?;
        let created: Vec<(String, i64)> = sqlx::query_as(
            "SELECT aggregate_id, MAX(sequence) FROM events \
             WHERE event_type = 'TriggerCreated' AND sequence > $1 \
             GROUP BY aggregate_id",
        )
        .bind(after)
        .fetch_all(pool)
        .await?;

        let mut exclusions = Self {
            created: created.into_iter().collect(),
            known,
            ..Self::default()
        };
        for (trigger_id, source_event_id) in fired {
            exclusions
                .fired
                .entry(source_event_id)
                .or_default()
                .insert(trigger_id);
        }
        Ok(exclusions)
    }

    fn for_row(
        &self,
        row: &StoredTriggerCarrier,
        trigger_configs: &TriggerConfigs,
    ) -> HashSet<String> {
        let mut excluded = self.fired.get(&row.id).cloned().unwrap_or_default();
        excluded.extend(
            self.created
                .iter()
                .filter(|(_, created_at)| **created_at > row.sequence)
                .map(|(trigger_id, _)| trigger_id.clone()),
        );
        excluded.extend(
            trigger_configs
                .read()
                .unwrap()
                .keys()
                .filter(|id| !self.known.contains(*id))
                .cloned(),
        );
        excluded
    }
}

#[cfg(test)]
#[path = "bus_subscriber_tests.rs"]
mod tests;
