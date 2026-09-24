//! A coding agent's *background task*: work that has to outlive the turn.
//!
//! A coding agent cannot keep a job running past its own turn, because the
//! engine tears its process group down when the turn ends. So the engine runs
//! the job itself, through the registry behind the chat agent's
//! `run_bash_background`, and arms an *event wait* on its
//! `BackgroundBashCompleted`. The agent ends its turn, and the wait re-opens the
//! thread when the job finishes. The agent reaches this through `lucidos
//! background-task` over `/api/v1/threads/:thread_id/background-tasks`.

use uuid::Uuid;

use crate::engine::LucidosEngine;
use crate::llm::tools::BG_MAX_TIMEOUT_SECS;

/// What starting a background task did.
#[derive(Debug)]
pub(crate) enum BackgroundTaskStart {
    /// Running, and an event wait re-opens the thread when it finishes.
    Watched { task_id: String, timeout_secs: u64 },
    /// Running, but nothing will re-open the thread when it finishes: a
    /// subscription cap or an error refused the wait.
    Unwatched { task_id: String, timeout_secs: u64 },
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

/// The thread events that end a thread's work, and so its background tasks.
fn ends_the_threads_work(event: &crate::engine::thread_events::ThreadEvent) -> bool {
    use crate::engine::thread_events::ThreadEvent;
    matches!(
        event,
        ThreadEvent::ThreadArchived | ThreadEvent::ThreadDiscarded { .. }
    )
}

impl LucidosEngine {
    /// Start `command` as a background task in this thread's worktree, and arm
    /// the wait that re-opens the thread when it finishes.
    ///
    /// `timeout_secs` defaults to the ceiling rather than to the chat tool's
    /// ten minutes. A coding agent reaches for this exactly when the work is
    /// long, and a watchdog firing mid-suite reads as a failure.
    pub(crate) async fn start_background_task_for_agent(
        &self,
        thread_id: Uuid,
        command: &str,
        timeout_secs: Option<u64>,
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

        let env_vars = self.build_agent_task_env_vars(thread_id).await;
        let (task_id, _started_at) = self
            .start_background_task(thread_id, command, timeout_secs, &cwd, &env_vars)
            .await?;

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

    /// Stop one of this thread's running background tasks. Its completion is
    /// still recorded, as killed, and delivered to the thread's wait.
    pub(crate) async fn stop_background_task_for_agent(
        &self,
        thread_id: Uuid,
        task_id: &str,
    ) -> Result<String, String> {
        self.refuse_another_threads_task(thread_id, task_id).await?;
        if self.bash_background.kill(task_id).await {
            Ok(format!("Stopped background task {task_id}."))
        } else {
            Err(format!(
                "Background task {task_id} is not running on this thread. It may already \
                 have finished: read its output instead."
            ))
        }
    }

    /// Kill every running background task of a thread whose work is being
    /// thrown away. Each killed task still records its completion, as killed.
    ///
    /// Called from every place a thread's work ends: `stop_agent` for a live
    /// session, the user's change discards in `change_ops::discard`, and the bus
    /// subscriber below for an archived or discarded thread.
    pub(crate) async fn abandon_background_tasks(&self, thread_id: Uuid, why: &str) {
        let killed = self.bash_background.kill_for_thread(thread_id).await;
        if killed > 0 {
            crate::log!(
                "[BackgroundTask] {why} killed {killed} background task(s) of thread {thread_id}"
            );
        }
    }

    /// Kill a thread's background tasks when the bus says the thread ended.
    ///
    /// On the bus rather than at each endpoint, for the reason
    /// `cancel_waits_ended_by` gives. A thread is archived by the button, by a
    /// parent's cascade and by an agent tool. The persisted event is the one
    /// point they all share. An archive usually finds no live session, so
    /// `stop_agent` never runs for it.
    pub fn start_background_task_reaper(self: &std::sync::Arc<Self>) {
        let mut rx = self.event_bus.subscribe();
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(emitted) => {
                        if let crate::engine::event_bus::BusEvent::Thread {
                            thread_id, event, ..
                        } = &emitted.typed
                        {
                            if ends_the_threads_work(event) {
                                engine
                                    .abandon_background_tasks(*thread_id, event.event_type())
                                    .await;
                            }
                        }
                    }
                    // A lagged burst can hide an archive, and that task then
                    // runs to its own watchdog. Bounded, so logged, not fatal.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        crate::log!("[BackgroundTask] reaper lagged by {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
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
