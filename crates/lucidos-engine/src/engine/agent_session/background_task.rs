//! A coding agent's *background task*: work that has to outlive the turn.
//!
//! A coding agent cannot keep a job running past its own turn, because the
//! engine stops every process the agent started when the turn ends
//! (`runtime::agent_run_marker`). So the engine runs
//! the job itself, through the registry behind the chat agent's
//! `run_bash_background`, and arms an *event wait* on its
//! `BackgroundBashCompleted`. The agent ends its turn, and the wait re-opens the
//! thread when the job finishes. The agent reaches this through `lucidos
//! background-task` over `/api/v1/threads/:thread_id/background-tasks`.

use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EmittedEvent, LagReplay, ReplayScope};
use crate::engine::event_wait::StoppedTask;
use crate::engine::tools::bash_background::{BackgroundBashRegistry, CompletionDelivery, TaskSpec};
use crate::engine::LucidosEngine;
use crate::llm::tools::BG_MAX_TIMEOUT_SECS;

/// The longest one `lucidos background-task wait` blocks, in seconds. The
/// Claude Code prompt cache lives 5 minutes or 1 hour from the last request's
/// start, so the next request lands inside either (ADR 0470).
pub(crate) const AGENT_WAIT_MAX_SECS: u64 = 230;

/// What starting a background task did.
#[derive(Debug)]
pub(crate) enum BackgroundTaskStart {
    /// Running, and an event wait re-opens the thread when it finishes.
    Watched { task_id: String, timeout_secs: u64 },
    /// Running, but nothing will re-open the thread when it finishes: a
    /// subscription cap or an error refused the wait.
    Unwatched { task_id: String, timeout_secs: u64 },
    /// Running with no event wait, because the agent asked to poll it with
    /// `lucidos background-task wait`.
    Polled { task_id: String, timeout_secs: u64 },
    /// Finished before a wait was needed. `output` is the task's result, in the
    /// same JSON shape `output` returns.
    Finished { task_id: String, output: String },
}

/// How a just-started task stands once the engine has tried to arm its wait.
#[derive(Debug, PartialEq, Eq)]
enum Settled {
    Watched,
    Unwatched,
    AlreadyFinished,
}

/// Decide from the arming result and a later liveness read.
///
/// Covered means a wait was armed while the task was running, so its completion
/// is delivered even if it has landed since. A task that finished before the
/// arming was never covered, and its result goes back inline instead. Asking
/// "covered?" first is what keeps one completion from arriving twice.
fn settle_start(task_id: &str, covered: &[String], still_running: bool) -> Settled {
    if covered.iter().any(|id| id == task_id) {
        Settled::Watched
    } else if still_running {
        Settled::Unwatched
    } else {
        Settled::AlreadyFinished
    }
}

/// How long one `wait` blocks: the caller's ask, never past the cap.
fn agent_wait_secs(max_secs: Option<u64>) -> u64 {
    max_secs
        .unwrap_or(AGENT_WAIT_MAX_SECS)
        .min(AGENT_WAIT_MAX_SECS)
}

/// What `lucidos background-task stop` prints, for the agent reading it.
pub(crate) fn stopped_message(stopped: &StoppedTask) -> String {
    let mut message = format!(
        "Stopped background task {} ({}). Its completion will not re-open this thread.",
        stopped.task_id, stopped.label
    );
    message.push_str(&ended_waits_note(&stopped.ended_with_others));
    message
}

/// The note for waits a stop ended whole because they watched more than the
/// stopped task. Empty when there were none.
pub(crate) fn ended_waits_note(ended_with_others: &[String]) -> String {
    if ended_with_others.is_empty() {
        return String::new();
    }
    format!(
        " It also ended your wait for {}, which watched more than this task. Re-arm it \
         if you still need the rest.",
        ended_with_others.join("; ")
    )
}

/// Kill a thread's running background tasks, and log how many.
async fn kill_threads_tasks(registry: &BackgroundBashRegistry, thread_id: Uuid, why: &str) {
    let killed = registry.kill_for_thread(thread_id).await;
    if killed > 0 {
        crate::log!(
            "[BackgroundTask] {why} killed {killed} background task(s) of thread {thread_id}"
        );
    }
}

/// The event types that end a thread's work. [`ends_the_threads_work`] reads
/// the same two.
const WORK_ENDING_EVENT_TYPES: [&str; 2] = ["ThreadArchived", "ThreadDiscarded"];

/// Apply one read of the bus to the running background tasks. A lag hands
/// back the archives and discards it dropped. Returns `false` once the bus
/// closes.
async fn reap_ended_work(
    received: Result<EmittedEvent, RecvError>,
    replay: &mut LagReplay,
    registry: &BackgroundBashRegistry,
    pool: &sqlx::PgPool,
) -> bool {
    let Some(frames) = replay.read(received, pool).await else {
        return false;
    };
    for emitted in frames {
        if let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        {
            if ends_the_threads_work(event) {
                kill_threads_tasks(registry, *thread_id, event.event_type()).await;
            }
        }
    }
    true
}

/// The thread events that end a thread's work, and so its background tasks.
fn ends_the_threads_work(event: &crate::engine::thread_events::ThreadEvent) -> bool {
    use crate::engine::thread_events::ThreadEvent;
    matches!(
        event,
        ThreadEvent::ThreadArchived | ThreadEvent::ThreadDiscarded { .. }
    )
}

impl LucidosEngine {
    /// Start `command` as a background task in this thread's worktree. Under
    /// [`CompletionDelivery::EventWait`], arm the wait that re-opens the thread
    /// when it finishes. The wait names the task by `description`, or by its
    /// command when there is none. Under [`CompletionDelivery::Poll`] nothing
    /// is armed: the agent polls with `lucidos background-task wait`.
    ///
    /// `timeout_secs` defaults to the ceiling rather than to the chat tool's
    /// ten minutes. A coding agent reaches for this exactly when the work is
    /// long, and a watchdog firing mid-suite reads as a failure.
    pub(crate) async fn start_background_task_for_agent(
        &self,
        thread_id: Uuid,
        command: &str,
        description: Option<&str>,
        timeout_secs: Option<u64>,
        delivery: CompletionDelivery,
    ) -> Result<BackgroundTaskStart, String> {
        let command = command.trim();
        if command.is_empty() {
            return Err("A background task needs a command.".to_string());
        }
        // The agent's own permission check saw only `lucidos background-task
        // run -- …`, and its Bash guard read head `lucidos`. This is the one
        // place that reads the real command.
        if let Some(reason) = crate::engine::command_guard::catastrophic_reason_for_command(command)
        {
            return Err(format!("Refused: the command is {reason}."));
        }
        let cwd = {
            let sessions = self.agent_sessions.lock().await;
            sessions
                .get(&thread_id)
                .and_then(|s| s.worktree_path.clone())
        };
        let Some(cwd) = cwd else {
            return Err(
                "This thread has no live coding-agent session with a worktree, so \
                 there is nowhere to run a background task. Run it in the foreground \
                 instead."
                    .to_string(),
            );
        };
        let timeout_secs = timeout_secs
            .unwrap_or(BG_MAX_TIMEOUT_SECS)
            .clamp(1, BG_MAX_TIMEOUT_SECS);

        let env_vars = self.build_agent_task_env_vars(thread_id, &cwd).await;
        let spec = TaskSpec {
            timeout_secs,
            description,
            delivery,
            inherited: crate::core::InheritedSecrets::Keep,
        };
        let (task_id, _started_at) = self
            .start_background_task(thread_id, command, &cwd, &env_vars, spec)
            .await?;
        if delivery == CompletionDelivery::Poll {
            return Ok(BackgroundTaskStart::Polled {
                task_id,
                timeout_secs,
            });
        }

        let covered = self.arm_wait_for_running_background_tasks(thread_id).await;
        let still_running = self.bash_background.is_running(&task_id).await;
        match settle_start(&task_id, &covered, still_running) {
            Settled::Watched => Ok(BackgroundTaskStart::Watched {
                task_id,
                timeout_secs,
            }),
            Settled::Unwatched => Ok(BackgroundTaskStart::Unwatched {
                task_id,
                timeout_secs,
            }),
            Settled::AlreadyFinished => {
                let output = self
                    .background_task_output_for_agent(thread_id, &task_id)
                    .await?;
                Ok(BackgroundTaskStart::Finished { task_id, output })
            }
        }
    }

    /// The output of one of this thread's background tasks: what arrived since
    /// the last read while it runs, and its final record once it has finished.
    pub(crate) async fn background_task_output_for_agent(
        &self,
        thread_id: Uuid,
        task_id: &str,
    ) -> Result<String, String> {
        self.refuse_another_threads_task(thread_id, task_id).await?;
        self.execute_bash_output_tool(&serde_json::json!({ "task_id": task_id }), thread_id)
            .await
    }

    /// Block until one of this thread's background tasks finishes, or
    /// `max_secs` pass, then return what `output` returns: the new output, and
    /// the final record once finished. The cap is clamped to
    /// [`AGENT_WAIT_MAX_SECS`] whatever the caller asks. A timed-out wait
    /// leaves the task running.
    pub(crate) async fn background_task_wait_for_agent(
        &self,
        thread_id: Uuid,
        task_id: &str,
        max_secs: Option<u64>,
    ) -> Result<String, String> {
        self.refuse_another_threads_task(thread_id, task_id).await?;
        let wait = std::time::Duration::from_secs(agent_wait_secs(max_secs));
        self.drain_background_output(task_id, wait, thread_id).await
    }

    /// Stop one of this thread's running background tasks. Its completion is
    /// still recorded, as killed, but it does not re-open this thread: the
    /// stop stands the thread's own waits on it down first (ADR 0369).
    pub(crate) async fn stop_background_task_for_agent(
        &self,
        thread_id: Uuid,
        task_id: &str,
    ) -> Result<String, String> {
        self.refuse_another_threads_task(thread_id, task_id).await?;
        match self.stop_background_task_as(thread_id, task_id).await {
            Some(stopped) => Ok(stopped_message(&stopped)),
            None => Err(format!(
                "Background task {task_id} is not running on this thread, or a stop is \
                 already on its way. It may have finished: read its output instead."
            )),
        }
    }

    /// Kill every running background task of a thread whose work is being
    /// thrown away. Each killed task still records its completion, as killed.
    ///
    /// Called from every place a thread's work ends: `stop_agent` for a live
    /// session, the user's change discards in `change_ops::discard`, and the bus
    /// subscriber below for an archived or discarded thread.
    pub(crate) async fn abandon_background_tasks(&self, thread_id: Uuid, why: &str) {
        kill_threads_tasks(&self.bash_background, thread_id, why).await;
    }

    /// Kill a thread's background tasks when the bus says the thread ended.
    ///
    /// On the bus rather than at each endpoint, for the reason
    /// `cancel_waits_ended_by` gives. A thread is archived by the button, by a
    /// parent's cascade and by an agent tool. The persisted event is the one
    /// point they all share. An archive usually finds no live session, so
    /// `stop_agent` never runs for it.
    pub fn start_background_task_reaper(self: &std::sync::Arc<Self>) {
        let (mut rx, mut replay) = LagReplay::subscribe(
            &self.event_bus,
            ReplayScope::Workspace,
            Some(WORK_ENDING_EVENT_TYPES.to_vec()),
        );
        let engine = self.clone();
        tokio::spawn(async move {
            while reap_ended_work(
                rx.recv().await,
                &mut replay,
                &engine.bash_background,
                engine.pool(),
            )
            .await
            {}
        });
    }

    /// A task id is an address, and this thread may only use its own. A task
    /// the registry no longer holds passes: the event-store fallback in
    /// `execute_bash_output_tool` is already scoped to this thread.
    async fn refuse_another_threads_task(
        &self,
        thread_id: Uuid,
        task_id: &str,
    ) -> Result<(), String> {
        match self.bash_background.spawned_by(task_id, thread_id).await {
            Some(false) => Err(format!("No background task {task_id} on this thread.")),
            Some(true) | None => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "background_task_tests.rs"]
mod tests;
