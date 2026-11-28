use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::keep_alive::{
    unattended_trigger_sql, CacheKeepAliveMode, KeepWarmKinds, WaitAgent, WaitType,
};
use super::{describe_subscriptions, LiveWait};
use crate::engine::bash_background::TaskProgress;
use crate::engine::thread_events::{WakeMode, WakeSource};
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::runtime::CodingAgent;

const MISSES_SHOWN: i64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct PendingWake {
    pub agent: WaitAgent,
    pub waits: Vec<PendingWait>,
    pub children: Vec<PendingChild>,
    pub tasks: Vec<TaskProgress>,
    pub history: Vec<String>,
    pub misses: Vec<MissedEstimate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingWait {
    pub reason: String,
    pub watches: String,
    pub armed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingChild {
    pub title: String,
    pub task: String,
    pub awaiting_user: bool,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct MissedEstimate {
    pub stated_at: DateTime<Utc>,
    pub expected_at: DateTime<Utc>,
    pub source: sqlx::types::Json<WakeSource>,
    pub reason: String,
}

#[derive(Debug, sqlx::FromRow)]
struct ThreadRow {
    is_coding_agent: bool,
    coding_agent: Option<String>,
    unattended_trigger: bool,
}

impl ThreadRow {
    fn agent(&self) -> Option<WaitAgent> {
        match (self.is_coding_agent, self.coding_agent.as_deref()) {
            (false, _) => Some(WaitAgent::Lucidos),
            (true, Some(agent)) if agent == CodingAgent::Codex.as_str() => None,
            (true, _) => Some(WaitAgent::ClaudeCode),
        }
    }
}

#[derive(sqlx::FromRow)]
struct ChildRow {
    title: Option<String>,
    first_message: Option<String>,
    status: String,
    created_at: DateTime<Utc>,
}

pub(super) fn keep_warm_waits(waits: Vec<LiveWait>) -> Vec<PendingWait> {
    waits
        .into_iter()
        .filter(|w| KeepWarmKinds::of(w.on.iter().map(|s| s.event_type.as_str())).is_some())
        .map(|w| PendingWait {
            watches: describe_subscriptions(&w.on),
            reason: w.reason,
            armed_at: w.armed_at,
            expires_at: w.expires_at,
        })
        .collect()
}

pub(super) async fn read_pending_wake(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    waits: Vec<PendingWait>,
) -> Result<Option<PendingWake>, sqlx::Error> {
    let Some(thread) = sqlx::query_as::<_, ThreadRow>(&format!(
        "SELECT ts.is_coding_agent, ts.coding_agent, {} AS unattended_trigger \
           FROM thread_summaries ts WHERE ts.thread_id = $1",
        unattended_trigger_sql("ts")
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let Some(agent) = thread.agent().filter(|_| !thread.unattended_trigger) else {
        return Ok(None);
    };
    let children: Vec<PendingChild> = sqlx::query_as::<_, ChildRow>(
        "SELECT title, first_message, status, created_at FROM thread_summaries \
          WHERE parent_thread_id = $1 AND wake_mode IS DISTINCT FROM $2 \
            AND status IN ($3, $4) \
          ORDER BY created_at",
    )
    .bind(thread_id)
    .bind(WakeMode::NoWake.as_str())
    .bind(ThreadStatus::Running.as_str())
    .bind(ThreadStatus::WaitingForUserAnswer.as_str())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| PendingChild {
        title: row.title.unwrap_or_default(),
        task: row.first_message.unwrap_or_default(),
        awaiting_user: row.status == ThreadStatus::WaitingForUserAnswer.as_str(),
        started_at: row.created_at,
    })
    .collect();
    if waits.is_empty() && children.iter().all(|c| c.awaiting_user) {
        return Ok(None);
    }
    let Some(oldest) = waits
        .iter()
        .map(|w| w.armed_at)
        .chain(children.iter().map(|c| c.started_at))
        .min()
    else {
        return Ok(None);
    };
    let misses = sqlx::query_as::<_, MissedEstimate>(
        "SELECT created AS stated_at, (payload->>'at')::timestamptz AS expected_at, \
                payload->'source' AS source, COALESCE(payload->>'reason', '') AS reason \
           FROM events \
          WHERE thread_id = $1 AND event_type = 'WakeExpected' AND created >= $2 \
            AND (payload->>'at')::timestamptz < now() \
          ORDER BY sequence DESC LIMIT $3",
    )
    .bind(thread_id)
    .bind(oldest)
    .bind(MISSES_SHOWN)
    .fetch_all(pool)
    .await?;
    Ok(Some(PendingWake {
        agent,
        waits,
        children,
        tasks: Vec::new(),
        history: Vec::new(),
        misses,
    }))
}

impl PendingWake {
    fn wait_types(&self, waits: &[LiveWait]) -> Vec<(String, WaitType)> {
        let mut types: Vec<(String, WaitType)> = Vec::new();
        let from_waits = waits.iter().filter_map(|w| {
            let kinds = KeepWarmKinds::of(w.on.iter().map(|s| s.event_type.as_str()))?;
            Some((format!("Waits on {}", describe_subscriptions(&w.on)), kinds))
        });
        let from_children = (!self.children.is_empty()).then(|| {
            (
                "Sub-threads reporting".to_string(),
                KeepWarmKinds::child_finish(),
            )
        });
        for (label, kinds) in from_waits.chain(from_children) {
            let wait_type = WaitType::Events(kinds);
            if !types.iter().any(|(_, seen)| *seen == wait_type) {
                types.push((label, wait_type));
            }
        }
        types
    }
}

impl crate::engine::LucidosEngine {
    pub async fn pending_wake(&self, thread_id: Uuid) -> Option<PendingWake> {
        if self.keep_alive_mode().await != CacheKeepAliveMode::On {
            return None;
        }
        let live = self.live_waits.for_thread(thread_id).await;
        let mut pending = read_pending_wake(&self.pool, thread_id, keep_warm_waits(live.clone()))
            .await
            .unwrap_or_else(|e| {
                crate::log_legacy!("[ExpectedWake] Pending work of {thread_id} unreadable: {e}");
                None
            })?;
        let durations = self.keep_alive_durations.current();
        pending.history = pending
            .wait_types(&live)
            .into_iter()
            .filter_map(|(label, wait_type)| {
                let history = durations.history(pending.agent, wait_type)?;
                Some(format!("{label}: {}", history.describe()))
            })
            .collect();
        pending.tasks = self.bash_background.progress_for_thread(thread_id).await;
        Some(pending)
    }
}

#[cfg(test)]
#[path = "pending_wake_tests.rs"]
mod tests;
