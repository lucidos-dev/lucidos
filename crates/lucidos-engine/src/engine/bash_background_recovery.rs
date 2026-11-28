use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::bash_background::{command_prefix, BackgroundBashRegistry, CompletionRecord};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbandonedTask {
    pub thread_id: Uuid,
    pub task_id: String,
    pub command: String,
    pub started_at: DateTime<Utc>,
}

/// INVARIANT: What `stderr` says on a completion the BOOT sweep wrote.
///
/// It goes in a stream rather than in a new field because a stream is what
/// every reader already renders. `bash_output` returns the two verbatim, and an
/// event-wait delivery pretty-prints the whole payload into the thread. The
/// `abandoned` flag beside it carries the same fact structurally.
///
/// **It does not claim the child is dead**, and that restraint is the point.
/// This path runs after a crash. No destructor ran, so `kill_on_drop` never
/// fired, and a child holding no live pipe write is reparented to init and
/// carries on. Telling an agent "nothing is running, re-run it" would start a
/// second release beside the first.
const ENGINE_CRASHED_NOTE: &str =
    "[the engine stopped while this task was running, so no exit status was ever reaped and \
     the output was lost with the process. The child may or may not have outlived it: check \
     before starting the same work again]";

/// INVARIANT: What `stderr` says on a completion the TEARDOWN wrote.
///
/// Stronger than [`ENGINE_CRASHED_NOTE`] on the one point it can be: this path
/// killed the task and waited for the reap, so the output above it is
/// everything the task ever wrote.
///
/// The kill reaches the task's whole process group, so a pipeline or a command
/// list stops with its shell. **It still stops short of "nothing is running".**
/// A process that left the group, through `setsid` or a browser detaching
/// itself, is out of reach. Telling an agent to re-run a release that one of
/// those carried on would start a second beside the first.
const ENGINE_STOPPED_NOTE: &str = "[the engine was shutting down, so it killed this task and \
                                   every process in its group. The work did not finish. A \
                                   process that detached into its own session may still be \
                                   running: check before starting the same work again]";

/// INVARIANT: Tasks with a `BackgroundBashStarted` and no matching `BackgroundBashCompleted`.
///
/// **Unbounded in time, deliberately.** An earlier draft settled only tasks
/// still inside their own `timeout_secs`, to keep the first boot after this
/// shipped from writing rows onto long-finished threads. That traded a one-time
/// tidiness for a permanent hole. An engine down past a task's watchdog
/// deadline left it unsettled forever. Its wait then expired blaming the
/// deadline, which is the stall this module removes.
///
/// The backfill is harmless by comparison. `BackgroundBashCompleted` is
/// `metadata` to the projection, so it bumps no activity and moves no thread.
/// Every wait old enough to be affected was resolved by the deadline sweep long
/// ago.
///
/// `s.aggregate = 'thread'` is load-bearing for the same reason it is in
/// `event_wait::LIVE_WAITS_SQL`. On a domain event `aggregate_id` holds the
/// event type name, which fails the cast and takes the whole query with it.
const ABANDONED_TASKS_SQL: &str = "\
    SELECT s.aggregate_id::uuid, \
           s.payload->>'task_id', \
           COALESCE(s.payload->>'command', ''), \
           s.payload->>'started_at', \
           s.created \
    FROM events s \
    JOIN thread_summaries t ON t.thread_id = s.aggregate_id::uuid \
    WHERE s.aggregate = 'thread' \
      AND s.event_type = 'BackgroundBashStarted' \
      AND s.payload->>'task_id' IS NOT NULL \
      AND t.state IS DISTINCT FROM 'discarded' \
      AND NOT EXISTS ( \
          SELECT 1 FROM events c \
          WHERE c.aggregate = 'thread' \
            AND c.aggregate_id = s.aggregate_id \
            AND c.event_type = 'BackgroundBashCompleted' \
            AND c.payload->>'task_id' = s.payload->>'task_id' \
      ) \
    ORDER BY s.sequence";

type AbandonedRow = (Uuid, String, String, Option<String>, DateTime<Utc>);

/// INVARIANT: How long the teardown waits for ALL its killed children to be reaped.
///
/// The watchdog SIGKILLs and awaits the child, then joins its two drain readers
/// under a 2 s deadline of their own. This only has to clear that, and the
/// kills all went out together, so one budget covers them.
///
/// Kept tight rather than generous. The whole shutdown is force-killed at 15 s,
/// and `shutdown_agent_sessions` after this already spends up to 10 s draining
/// coding agents. Overrunning costs those their clean teardown. Giving up early
/// costs one task's buffered output, which the boot sweep then records
/// without.
pub const REAP_WAIT: Duration = Duration::from_secs(3);

pub async fn abandoned_background_tasks(
    pool: &sqlx::PgPool,
) -> Result<Vec<AbandonedTask>, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<AbandonedRow> = sqlx::query_as(ABANDONED_TASKS_SQL).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(
            |(thread_id, task_id, command, started_at, created)| AbandonedTask {
                thread_id,
                task_id,
                command: command_prefix(&command),
                started_at: started_at
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                    .unwrap_or(created),
            },
        )
        .collect())
}

pub async fn task_start_time(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    task_id: &str,
) -> Result<Option<DateTime<Utc>>, Box<dyn std::error::Error + Send + Sync>> {
    let row: Option<(Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT payload->>'started_at', created FROM events \
         WHERE aggregate = 'thread' \
           AND event_type = 'BackgroundBashStarted' \
           AND aggregate_id = $1 \
           AND payload->>'task_id' = $2 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(task_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(started_at, created)| {
        started_at
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or(created)
    }))
}

pub fn completion_event(task_id: String, record: CompletionRecord) -> ThreadEvent {
    let stderr = super::bash_background::finalize_drain(&record.stderr, record.stderr_dropped);
    let outcome = record.outcome.as_reported(record.abandoned);
    ThreadEvent::BackgroundBashCompleted {
        task_id,
        command: record.command,
        exit_code: outcome.exit_code(),
        signal: outcome.signal(),
        stdout: super::bash_background::finalize_drain(&record.stdout, record.stdout_dropped),
        stderr: if record.abandoned {
            with_note(stderr, ENGINE_STOPPED_NOTE)
        } else {
            stderr
        },
        started_at: record.started_at,
        finished_at: record.finished_at,
        timed_out: record.timed_out,
        killed: record.killed,
        abandoned: record.abandoned,
    }
}

/// INVARIANT: Drop any candidate the CURRENT process is still running.
///
/// Always a no-op at boot, where the registry starts empty. It is the guard
/// that makes the sweep safe to call from anywhere else: settling a LIVE task
/// would resolve its wait mid-flight, and make the next drain report a
/// finished task that is still building.
///
/// A free function taking the registry, so the guard is testable. Inline in the
/// engine method it could be deleted with the whole suite still green, which is
/// the shape of an invariant nothing holds.
pub(super) async fn settleable(
    registry: &BackgroundBashRegistry,
    candidates: Vec<AbandonedTask>,
) -> Vec<AbandonedTask> {
    let mut keep = Vec::with_capacity(candidates.len());
    for task in candidates {
        if !registry.is_running(&task.task_id).await {
            keep.push(task);
        }
    }
    keep
}

/// INVARIANT: The completion event for a task nobody watched end.
///
/// `exit_code` and `signal` are both `None`, which `TaskOutcome::from_persisted`
/// reads back as `Unknown` and renders as "exit code unknown". No status was
/// reaped, and inventing one would make a killed task look like a clean exit.
///
/// `finished_at` is when the loss was RECORDED, never when the child died,
/// because nothing observed the death. On the boot path it therefore spans the
/// engine's downtime too. The `abandoned` flag is what tells a reader not to
/// take the span as runtime.
fn abandoned_completion(
    task_id: String,
    command: String,
    started_at: DateTime<Utc>,
    note: &str,
) -> ThreadEvent {
    ThreadEvent::BackgroundBashCompleted {
        task_id,
        command,
        exit_code: None,
        signal: None,
        stdout: String::new(),
        stderr: note.to_string(),
        started_at,
        finished_at: Utc::now(),
        timed_out: false,
        killed: false,
        abandoned: true,
    }
}

impl LucidosEngine {
    /// INVARIANT: Settle every task the previous process abandoned. Boot only.
    ///
    /// **Ordered between the lost-re-entry sweep and the wait rebuild**, and
    /// both sides matter. Its emit is an event on the thread, so running it
    /// first buries a stranded re-entry's anchor. The rebuild's catch-up scan
    /// is what turns the completion into a re-opened thread, so it has to come
    /// after. `main.rs` states the same at the call site.
    ///
    /// Returns how many it settled. A failure is logged and swallowed, since a
    /// boot must not die on it, and the log names the cost rather than noting a
    /// skip: an unsettled task is a thread that sits until its own deadline.
    pub async fn settle_abandoned_background_tasks(&self) -> usize {
        let abandoned = match abandoned_background_tasks(&self.pool).await {
            Ok(rows) => rows,
            Err(e) => {
                log_legacy!(
                    "[BashBg] Could not look for background tasks the last engine abandoned: {e}. \
                     Any thread watching one will sit until its subscription expires."
                );
                return 0;
            }
        };
        if abandoned.is_empty() {
            return 0;
        }

        let mut settled = 0usize;
        for task in settleable(&self.bash_background, abandoned).await {
            let event = abandoned_completion(
                task.task_id.clone(),
                task.command,
                task.started_at,
                ENGINE_CRASHED_NOTE,
            );
            if self
                .emit_task_completion(task.thread_id, &task.task_id, event)
                .await
            {
                settled += 1;
            }
        }
        if settled > 0 {
            log_legacy!(
                "[BashBg] Settled {settled} background task(s) the last engine did not outlive"
            );
        }
        settled
    }

    /// INVARIANT: Record every task whose completion nobody has written yet, killing the
    /// ones still running first. Graceful teardown only.
    ///
    /// **Kills first, and that ordering is the whole correctness of this
    /// method.** The teardown around it awaits seconds of session and browser
    /// cleanup, and a command finishing inside that window really does finish.
    /// Recording "did not finish" beforehand is a claim about the future.
    /// `bash_output` reads the last row, so a release that succeeded would come
    /// back reported as abandoned. The output is then the real one, because the
    /// read happens after the watchdog joined the drain readers.
    ///
    /// It runs AFTER the teardown's boundary aborts, because it blocks for the
    /// reap. A device-attributed `ResponseAborted` that misses the supervisor's
    /// force-kill costs every in-flight thread its auto-resume.
    ///
    /// It does not start a turn on a dying engine. `begin_teardown` sets
    /// `is_shutting_down` first, and mechanism 5 in `event_wait/dispatcher.rs`
    /// makes every resolution path return early while that holds.
    pub async fn settle_running_background_tasks_at_teardown(&self) -> usize {
        let handed_over = self.bash_background.hand_over_at_teardown().await;
        if handed_over.is_empty() {
            return 0;
        }
        // INVARIANT: ONE deadline for the whole loop, not one per task. The kills all went
        // out together, so the reaps overlap, and a task that had already
        // finished costs nothing at all. A per-task budget would multiply by N
        // inside a shutdown the supervisor SIGKILLs at 15 s, and the sweeps
        // below this would never
        // run: in-flight threads would get no abort and Chrome would be
        // orphaned.
        let deadline = std::time::Instant::now() + REAP_WAIT;
        let mut settled = 0usize;
        for task in handed_over {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if !self
                .bash_background
                .wait_until_finished(&task.task_id, left)
                .await
            {
                log_legacy!(
                    "[BashBg] Task {} did not reap in time; the next boot will record it",
                    task.task_id
                );
                continue;
            }
            let Some(record) = self.bash_background.completion_record(&task.task_id).await else {
                settled += 1;
                continue;
            };
            let event = completion_event(task.task_id.clone(), record);
            if self
                .emit_task_completion(task.thread_id, &task.task_id, event)
                .await
            {
                settled += 1;
            }
        }
        if settled > 0 {
            log_legacy!("[BashBg] Recorded {settled} unfinished background task(s) at shutdown");
        }
        settled
    }

    async fn emit_task_completion(
        &self,
        thread_id: Uuid,
        task_id: &str,
        event: ThreadEvent,
    ) -> bool {
        match self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta::NONE,
            })
            .await
        {
            Ok(_) => true,
            Err(e) => {
                log_legacy!("[BashBg] Could not settle task {task_id} on thread {thread_id}: {e}");
                false
            }
        }
    }
}

fn with_note(stream: String, note: &str) -> String {
    if stream.is_empty() {
        return note.to_string();
    }
    format!("{stream}\n{note}")
}

#[cfg(test)]
#[path = "bash_background_recovery_tests.rs"]
mod tests;
