//! Settling a background task whose engine went away.
//!
//! A background task is a child of the engine process, tracked only in the
//! in-memory `BackgroundBashRegistry`. When the engine goes, the child goes
//! with it and nobody observes the exit. Nothing used to write
//! `BackgroundBashCompleted` then, so three readers failed silently: the
//! engine-armed wait in `event_wait/background_task.rs` sat until its own
//! deadline, `bash_output` reported `unknown task_id`, and a resumed coding
//! agent believed its build was still running.
//!
//! Two entry points. [`LucidosEngine::settle_running_background_tasks_at_teardown`]
//! kills each task, waits for the reap, and emits from the real final state.
//! [`LucidosEngine::settle_abandoned_background_tasks`] runs at boot and is the
//! fail-closed floor, for the SIGKILL, OOM, panic and power cut no hook sees.
//! Its selection is an anti-join over the event store, so the two cannot
//! double-write.
//!
//! Full reasoning:
//! `docs/plans/2026-08-26-a-background-task-always-reaches-a-terminal-event.md`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::bash_background::{command_prefix, BackgroundBashRegistry, CompletionRecord};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;

/// One task the event store says started and never finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbandonedTask {
    pub thread_id: Uuid,
    pub task_id: String,
    /// Already cut to the completion event's prefix. The `Started` row carries
    /// the whole command.
    pub command: String,
    pub started_at: DateTime<Utc>,
}

/// What `stderr` says on a completion the BOOT sweep wrote.
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

/// What `stderr` says on a completion the TEARDOWN wrote.
///
/// Stronger than [`ENGINE_CRASHED_NOTE`] on the one point it can be: this path
/// killed the task and waited for the reap, so the output above it is
/// everything the task ever wrote.
///
/// **It still does not promise the work stopped.** The kill is a single-pid
/// SIGKILL at the `bash -c` wrapper rather than at its process group. A
/// pipeline or a command list therefore leaves its real work reparented to
/// init, and the commands this exists for are exactly that shape. Telling an
/// agent to re-run one would start a second release beside the first.
const ENGINE_STOPPED_NOTE: &str = "[the engine was shutting down, so it killed this task. The \
                                   work did not finish. Only the task's own shell was signalled, \
                                   so anything it had piped or detached may still be running: \
                                   check before starting the same work again]";

/// Tasks with a `BackgroundBashStarted` and no matching `BackgroundBashCompleted`.
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

/// One [`ABANDONED_TASKS_SQL`] row: thread, task id, command, the payload's
/// `started_at` as text, and the row's own `created` as the fallback for it.
type AbandonedRow = (Uuid, String, String, Option<String>, DateTime<Utc>);

/// How long the teardown waits for ALL its killed children to be reaped.
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
const REAP_WAIT: Duration = Duration::from_secs(3);

/// Run [`ABANDONED_TASKS_SQL`] and parse the rows.
///
/// A free function on the pool, so the selection rule is testable against a
/// real database without standing up an engine.
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
                // The row's own `created` is the fallback, not a synthesized zero.
                // The two differ only by the emit, so it is right to the millisecond.
                started_at: started_at
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                    .unwrap_or(created),
            },
        )
        .collect())
}

/// When this thread spawned `task_id`, if it ever did.
///
/// Asked by `bash_output` where it has nothing to return, so a task with no
/// recorded outcome is not reported in the words of a typo. That path is
/// reached only when the id is in no registry and has no completion row.
///
/// Scoped to the calling thread, matching the completion lookup beside it. An
/// id belonging to another thread is not this thread's to explain.
///
/// `Err` rather than `None` on a database failure. A probe that could not run
/// is UNKNOWN, never a no (`.claude/rules/rust.md`). Collapsing the two here
/// tells the agent its id was a typo because a query timed out.
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

/// The `BackgroundBashCompleted` for a task this process reaped.
///
/// **One builder, because two callers race to be the one that emits**: the
/// task's own watchdog watcher, and the teardown once its kill is reaped. They
/// contend for `BackgroundBashRegistry::completion_record` and the winner
/// writes. Two builders would make the row's content depend on who won a lock.
/// The teardown loses that race routinely: it needs one lock acquisition just
/// to see the task finished before it can go back for the record.
///
/// An abandoned task reports no status, via `TaskOutcome::as_reported`, which
/// is the one place that rule lives.
pub(super) fn completion_event(task_id: String, record: CompletionRecord) -> ThreadEvent {
    let stderr = super::bash::finalize_drain(&record.stderr, record.stderr_dropped);
    let outcome = record.outcome.as_reported(record.abandoned);
    ThreadEvent::BackgroundBashCompleted {
        task_id,
        command: record.command,
        exit_code: outcome.exit_code(),
        signal: outcome.signal(),
        // Tail, not head. `bash_output` falls back to this payload once the
        // task is evicted, and the drain path it must agree with keeps the
        // tail. For a 40-minute build the last lines are the failure; the first
        // are `Compiling serde`.
        stdout: super::bash::finalize_drain(&record.stdout, record.stdout_dropped),
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

/// Drop any candidate the CURRENT process is still running.
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

/// The completion event for a task nobody watched end.
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
        // Not `killed`: that field means `bash_kill`, and a reader who sees it
        // concludes a person or an agent called the work off.
        killed: false,
        abandoned: true,
    }
}

impl LucidosEngine {
    /// Settle every task the previous process abandoned. Boot only.
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
                log!(
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
            log!("[BashBg] Settled {settled} background task(s) the last engine did not outlive");
        }
        settled
    }

    /// Record every task whose completion nobody has written yet, killing the
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
        // ONE deadline for the whole loop, not one per task. The kills all went
        // out together, so the reaps overlap, and a task that had already
        // finished costs nothing at all. A per-task budget would multiply by N inside a shutdown the
        // supervisor SIGKILLs at 15 s, and the sweeps below this would never
        // run: in-flight threads would get no abort and Chrome would be
        // orphaned.
        let deadline = std::time::Instant::now() + REAP_WAIT;
        let mut settled = 0usize;
        for task in handed_over {
            // A child that outlives the deadline leaves no record here. The
            // next boot's sweep is the floor under that, and writes the loss
            // without the output.
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if !self
                .bash_background
                .wait_until_finished(&task.task_id, left)
                .await
            {
                log!(
                    "[BashBg] Task {} did not reap in time; the next boot will record it",
                    task.task_id
                );
                continue;
            }
            // The one-shot gate. Its own watcher usually takes the record
            // first. It emits through the same builder from the same state, so
            // a `None` here means the work is already done.
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
            log!("[BashBg] Recorded {settled} unfinished background task(s) at shutdown");
        }
        settled
    }

    /// Emit one completion, reporting whether it landed. Shared by the two
    /// settle paths so they cannot come to disagree about how a failed write is
    /// handled: logged, counted as unsettled, and left to the boot sweep.
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
                log!("[BashBg] Could not settle task {task_id} on thread {thread_id}: {e}");
                false
            }
        }
    }
}

/// Append `note` after whatever the task managed to write.
fn with_note(stream: String, note: &str) -> String {
    if stream.is_empty() {
        return note.to_string();
    }
    format!("{stream}\n{note}")
}

#[cfg(test)]
#[path = "bash_background_recovery_tests.rs"]
mod tests;
