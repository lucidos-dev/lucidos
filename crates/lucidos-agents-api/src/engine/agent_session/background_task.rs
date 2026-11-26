use crate::engine::tools::bash::BashExt;
use std::path::PathBuf;

use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::engine::bash_background::{BackgroundBashRegistry, CompletionDelivery, TaskSpec};
use crate::engine::child_report_channel::ChildReport;
use crate::engine::event_bus::{BusEvent, EmittedEvent, LagReplay, ReplayScope};
use crate::engine::event_wait::{OwnerTurn, StoppedTask};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;

pub(crate) const AGENT_BG_MAX_TIMEOUT_SECS: u64 = 14_400;

pub(crate) const AGENT_BG_DEFAULT_TIMEOUT_SECS: u64 = 3_600;

fn agent_task_timeout_secs(asked: Option<u64>) -> u64 {
    asked
        .unwrap_or(AGENT_BG_DEFAULT_TIMEOUT_SECS)
        .clamp(1, AGENT_BG_MAX_TIMEOUT_SECS)
}

fn background_floor_refusal(found: crate::engine::command_guard::Catastrophe) -> String {
    use crate::engine::command_guard::Catastrophe;
    match found {
        Catastrophe::Proven(reason) => format!("Refused: the command is {reason}."),
        Catastrophe::Unproven(reason) => format!(
            "Refused: the command guard cannot rule out that this is {reason}. Run it in the \
             foreground instead, where the owner can allow it once after reviewing it."
        ),
    }
}

pub(crate) const AGENT_WAIT_MAX_SECS: u64 = 230;

#[derive(Debug)]
pub(crate) struct AgentWaitAnswer {
    pub output: String,
    pub child_reports: Vec<ChildReport>,
}

#[derive(Debug)]
pub(crate) enum BackgroundTaskStart {
    Watched { task_id: String, timeout_secs: u64 },
    Unwatched { task_id: String, timeout_secs: u64 },
    Polled { task_id: String, timeout_secs: u64 },
    Finished { task_id: String, output: String },
}

#[derive(Debug, PartialEq, Eq)]
enum Settled {
    Watched,
    Unwatched,
    AlreadyFinished,
}

fn settle_start(task_id: &str, covered: &[String], still_running: bool) -> Settled {
    if covered.iter().any(|id| id == task_id) {
        Settled::Watched
    } else if still_running {
        Settled::Unwatched
    } else {
        Settled::AlreadyFinished
    }
}

fn agent_wait_secs(max_secs: Option<u64>) -> u64 {
    max_secs
        .unwrap_or(AGENT_WAIT_MAX_SECS)
        .min(AGENT_WAIT_MAX_SECS)
}

pub(crate) fn stopped_message(stopped: &StoppedTask) -> String {
    let mut message = format!(
        "Stopped background task {} ({}). Its completion will not re-open this thread.",
        stopped.task_id, stopped.label
    );
    message.push_str(&ended_waits_note(&stopped.ended_with_others));
    message
}

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

async fn kill_threads_tasks(registry: &BackgroundBashRegistry, thread_id: Uuid, why: &str) {
    let killed = registry.kill_for_thread(thread_id).await;
    if killed > 0 {
        crate::log_legacy!(
            "[BackgroundTask] {why} killed {killed} background task(s) of thread {thread_id}"
        );
    }
}

const WORK_ENDING_EVENT_TYPES: [&str; 2] = ["ThreadArchived", "ThreadDiscarded"];

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

fn ends_the_threads_work(event: &crate::engine::thread_events::ThreadEvent) -> bool {
    use crate::engine::thread_events::ThreadEvent;
    matches!(
        event,
        ThreadEvent::ThreadArchived | ThreadEvent::ThreadDiscarded { .. }
    )
}

pub fn start_background_task_reaper(engine: &std::sync::Arc<LucidosEngine>) {
    engine.start_background_task_reaper();
}

#[easy_ext::ext(BackgroundTaskExt)]
pub(crate) impl LucidosEngine {
    async fn start_background_task_for_agent(
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
        let cwd = self.agent_worktree(thread_id).await?;
        if let Some(found) =
            crate::engine::command_guard::catastrophic_reason_for_command_at(command, Some(&cwd))
        {
            return Err(background_floor_refusal(found));
        }
        if crate::engine::command_guard::command_reads_secret(command) {
            return Err("Refused: background tasks cannot read Lucidos secrets.".to_string());
        }
        let timeout_secs = agent_task_timeout_secs(timeout_secs);

        let sandbox =
            crate::runtime::agent_sandbox::Sandbox::resolve(self.pool(), self.workspace_path())
                .await
                .map_err(|e| e.to_string())?;
        let env_vars = self
            .build_agent_task_env_vars(thread_id, &cwd, sandbox.compile_cache())
            .await?;
        let spec = TaskSpec {
            timeout_secs,
            description,
            delivery,
            inherited: crate::core::InheritedSecrets::Keep,
        };
        let (task_id, _started_at) = self
            .start_background_task(&sandbox, thread_id, command, &cwd, &env_vars, spec)
            .await?;
        if delivery == CompletionDelivery::Poll {
            return Ok(BackgroundTaskStart::Polled {
                task_id,
                timeout_secs,
            });
        }

        let covered = self
            .arm_wait_for_running_background_tasks(thread_id, OwnerTurn::Running)
            .await;
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

    async fn agent_worktree(&self, thread_id: Uuid) -> Result<PathBuf, String> {
        let sessions = self.agent_sessions.lock().await;
        sessions
            .get(&thread_id)
            .and_then(|s| s.worktree_path.clone())
            .ok_or_else(|| {
                "This thread has no live coding-agent session with a worktree, so there is \
                 nowhere to run a background task. Run it in the foreground instead."
                    .to_string()
            })
    }

    async fn background_task_output_for_agent(
        &self,
        thread_id: Uuid,
        task_id: &str,
    ) -> Result<String, String> {
        self.refuse_another_threads_task(thread_id, task_id).await?;
        self.execute_bash_output_tool(&serde_json::json!({ "task_id": task_id }), thread_id)
            .await
    }

    async fn background_task_wait_for_agent(
        &self,
        thread_id: Uuid,
        task_id: &str,
        max_secs: Option<u64>,
        actor: Option<MessageOrigin>,
    ) -> Result<AgentWaitAnswer, String> {
        self.refuse_another_threads_task(thread_id, task_id).await?;
        let wait = std::time::Duration::from_secs(agent_wait_secs(max_secs));
        let waiting = self.child_reports_waiting(thread_id).await;
        let output = match waiting.clone() {
            Some(mut waiting) => {
                let report_arrived = async {
                    if waiting.wait_for(|n| *n > 0).await.is_err() {
                        std::future::pending::<()>().await;
                    }
                };
                tokio::select! {
                    biased;
                    output = self.drain_background_output(task_id, wait, thread_id) => output,
                    () = report_arrived => {
                        self.drain_background_output(task_id, std::time::Duration::ZERO, thread_id)
                            .await
                    }
                }
            }
            None => self.drain_background_output(task_id, wait, thread_id).await,
        }?;
        let child_reports = match waiting {
            Some(waiting) if *waiting.borrow() > 0 => {
                self.take_child_reports(thread_id, actor).await
            }
            _ => Vec::new(),
        };
        Ok(AgentWaitAnswer {
            output,
            child_reports,
        })
    }

    async fn stop_background_task_for_agent(
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

    async fn abandon_background_tasks(&self, thread_id: Uuid, why: &str) {
        kill_threads_tasks(&self.bash_background, thread_id, why).await;
    }

    fn start_background_task_reaper(self: &std::sync::Arc<Self>) {
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

    /// INVARIANT: A task id is an address, and this thread may only use its own. A task
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
