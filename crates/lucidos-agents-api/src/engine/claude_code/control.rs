use super::*;
use crate::engine::agent_recovery::orphaned_branch_work::OrphanedBranchWorkExt;
use crate::engine::agent_recovery::recovery::AgentRecoveryExt;
use crate::engine::agent_session::background_task::BackgroundTaskExt;
use crate::engine::agent_session::spawn_claim::SpawnClaimExt;
use crate::engine::CcCommandsResult;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum RungPick {
    SetModel {
        model: String,
        #[serde(default)]
        effort: Option<String>,
    },
    SetReasoningEffort {
        effort: String,
    },
}

/// What [`LucidosEngine::record_rung_pick`] did with a pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RungPickOutcome {
    /// Recorded. The thread's next spawn runs on it.
    Recorded,
    /// Refused: a turn runs, and its rung stays fixed until it ends.
    TurnRunning,
}

/// How long a pick waits for a finished turn's idle exit, and how often it
/// re-checks meanwhile. The exit usually lands within milliseconds.
const IDLE_EXIT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const IDLE_EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(250);

fn turn_open(status: crate::engine::thread_lifecycle::ThreadStatus) -> bool {
    use crate::engine::thread_lifecycle::ThreadStatus;
    matches!(
        status,
        ThreadStatus::Running | ThreadStatus::WaitingForUserAnswer | ThreadStatus::Paused
    )
}

/// Whether a turn runs on a thread, so its rung is locked (ADR 0542). A turn
/// runs while it starts, works, or waits on a question, which the status
/// says. A live process counts too: a follow-up would reach it on its old
/// rung. A cache keep-alive's hidden turn does not count, because it records
/// no rung over the pick.
pub(crate) fn rung_locked(
    status: crate::engine::thread_lifecycle::ThreadStatus,
    session: Option<&crate::engine::AgentSession>,
) -> bool {
    turn_open(status) || session.is_some_and(|s| s.is_live() && !s.keep_alive)
}

/// A live process a pick waits on before it refuses. The client frees the
/// rung at the turn's Result, but the run loop still finishes the turn and
/// then runs the idle exit. Only that exit may end the process: it keeps it
/// for an input the agent still owes. An apply holds the process, so no exit
/// comes.
pub(crate) fn awaits_idle_exit(session: &crate::engine::AgentSession) -> bool {
    session.is_live() && !session.keep_alive && session.change_claim.is_none()
}

/// The thread-less half of `stop_agent`: stop every session in the map that
/// `stop_refusal` lets go. A request that stopped nothing reports the refusal.
pub(crate) fn stop_every_session(
    sessions: &mut HashMap<Uuid, crate::engine::AgentSession>,
    reason: StopReason,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if sessions.is_empty() {
        return Err("No Claude Code process is running".into());
    }
    let mut refusal = None;
    let mut stopped_any = false;
    for session in sessions.values_mut() {
        if let Some(why) = stop_refusal(session) {
            refusal = Some(why);
            continue;
        }
        session.pending_stop = Some(reason);
        session.stop.notify_one();
        stopped_any = true;
    }
    match refusal {
        Some(why) if !stopped_any => Err(why.into()),
        _ => Ok(()),
    }
}

/// What `interrupt_agent` does with the entry it found for a thread.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InterruptRoute {
    /// A turn is in flight and a loop is listening: fire the interrupt.
    Interrupt,
    /// The turn already reached its boundary: nothing to interrupt.
    AlreadyWaiting,
    /// No loop is listening: the session is exiting, or a phantom. Take the
    /// no-session fallback.
    NoLoop,
}

/// `is_waiting` alone cannot tell a live turn from an entry whose loop is gone,
/// so the interrupt asks `is_in_flight`. A turn that reached its boundary stays
/// "already waiting" whether or not its loop is still there: an interrupt
/// would stop nothing either way.
pub(crate) fn interrupt_route(session: &crate::engine::AgentSession) -> InterruptRoute {
    if matches!(session.phase, crate::engine::SessionPhase::Applying) {
        // No turn runs while an apply holds the thread.
        return InterruptRoute::AlreadyWaiting;
    }
    if session.is_in_flight() {
        InterruptRoute::Interrupt
    } else if session.is_waiting {
        InterruptRoute::AlreadyWaiting
    } else {
        InterruptRoute::NoLoop
    }
}

#[easy_ext::ext(ControlExt)]
pub(crate) impl LucidosEngine {
    async fn stop_agent(
        self: &Arc<Self>,
        reason: StopReason,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let reserved = match thread_id {
            Some(tid) => match self.agent_sessions.lock().await.get_mut(&tid) {
                Some(session) => {
                    if let Some(refusal) = stop_refusal(session) {
                        return Err(refusal.into());
                    }
                    session.pending_stop = Some(reason);
                    Some(session.stop.clone())
                }
                None => None,
            },
            None => None,
        };

        if let Some(tid) = thread_id.filter(|_| reason.abandons_background_tasks()) {
            self.abandon_background_tasks(tid, &format!("{reason:?}"))
                .await;
        }

        if let Some(tid) = thread_id {
            if let Some(stop) = reserved {
                stop.notify_one();
                Ok(())
            } else {
                if self.cancel_thread(tid, actor.clone()) {
                    return Ok(());
                }
                let discard = matches!(reason, StopReason::Discard);
                let stale_result = self
                    .end_stale_waiting_session(tid, discard, actor.clone())
                    .await;
                let settled = self
                    .settle_unclaimed_running_thread(tid, actor, SettleTerminal::StuckProjection)
                    .await?;
                if settled {
                    Ok(())
                } else {
                    stale_result
                }
            }
        } else {
            stop_every_session(&mut *self.agent_sessions.lock().await, reason)
        }
    }

    /// Snapshot a session's `pending_stop` reason. Encapsulates the lock
    /// acquisition so the run_session loop and its post-loop cleanup don't
    /// have to know which field carries the stop reason — flipping `bool`
    /// fields back and forth here used to be the source of the phantom
    /// `ResponseCanceled` regressions.
    async fn pending_stop_reason(&self, thread_id: Uuid) -> Option<StopReason> {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .and_then(|s| s.pending_stop)
    }

    /// The live session's question flushes, which a card's gate waits on.
    /// `None` with no session: then no loop has text still on its way.
    async fn question_flushes(
        &self,
        thread_id: Uuid,
    ) -> Option<std::sync::Arc<crate::engine::question_flushes::QuestionFlushes>> {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .map(|s| s.question_flushes.clone())
    }

    /// Read and clear the device that clicked Cancel on a live session (stamped
    /// by `interrupt_agent`). The run_session interrupt arm calls this so the
    /// emitted `ResponseCanceled.actor` records the cancelling device. Drained
    /// on read — a resumed session must not carry a stale actor into its next
    /// turn. The `agent_sessions` analog of `take_cancel_actor` (the chat
    /// path's `ThreadHandle.cancel_actor` slot in `active_threads`).
    async fn take_session_cancel_actor(&self, thread_id: Uuid) -> Option<MessageOrigin> {
        self.agent_sessions
            .lock()
            .await
            .get_mut(&thread_id)
            .and_then(|s| s.cancel_actor.take())
    }

    /// Read and clear the session's `redirect_cause`. The run_session interrupt
    /// arm calls this so the interrupted turn's `ResponseCanceled` carries the
    /// redirect's cause (neutral render) rather than `UserStop`. Drained on
    /// read, so a resumed session carries no stale cause into its next turn.
    /// The redirect analog of `take_session_cancel_actor`.
    async fn take_session_redirect_cause(
        &self,
        thread_id: Uuid,
    ) -> Option<crate::engine::thread_events::CancelCause> {
        self.agent_sessions
            .lock()
            .await
            .get_mut(&thread_id)
            .and_then(|s| s.redirect_cause.take())
    }

    /// Interrupt a running Claude Code session — sends control_request:interrupt to stop
    /// current work without killing the session (like pressing Esc in the CC terminal).
    /// The CC process stays alive and enters waiting state.
    ///
    /// `actor` flows onto the `ResponseCanceled` emitted by the no-session
    /// settle fallback so the panel reads "You" instead of "System".
    ///
    /// Fallbacks when no live Claude Code subprocess exists for `thread_id`:
    ///   1. Cancel the in-process agentic loop via the `active_threads` token
    ///      (handles run_thread-spawned chat threads and CC threads that are
    ///      mid-startup, before SessionStarted has registered an agent_session).
    ///      When the cancel lands, `run_session`'s `chat_cancel` arm emits the
    ///      terminal `ResponseCanceled` — skip the settle to avoid double-emit.
    ///   2. Only if the cancel had no entry to land on, settle so the UI
    ///      unsticks. `terminal` picks the event: `CanceledQuestion` when this
    ///      request cancel-stamped a pending question (the caller's own write
    ///      is what put the row at `running`), `StuckProjection` otherwise.
    ///      See [`SettleTerminal`].
    ///
    /// Unlike `stop_agent`, interrupt is always a real Cancel/Stop click — the
    /// frontend offers no "interrupt for Apply/Discard/Archive" path.
    ///
    /// Returns `Ok(true)` when a live turn was interrupted, an in-process loop
    /// canceled, or a stuck `running` projection settled — i.e. a terminal event
    /// is on its way. `Ok(false)` when nothing was interruptible (the settle
    /// found no stuck projection, or — for the thread-id-less variant — every
    /// session was already waiting). The handler folds this into the
    /// `{"canceled": bool}` response so the client can self-heal a stale view.
    /// The `SESSION_ALREADY_WAITING` `Err` is preserved for the single-thread
    /// idle-session race so the stop handler keeps its no-op-200 mapping.
    async fn interrupt_agent(
        &self,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
        terminal: SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut guard = self.agent_sessions.lock().await;

        if let Some(tid) = thread_id {
            // An entry no loop listens to is no session at all.
            if let Some(session) = guard
                .get_mut(&tid)
                .filter(|s| interrupt_route(s) != InterruptRoute::NoLoop)
            {
                if interrupt_route(session) == InterruptRoute::AlreadyWaiting {
                    return Err(SESSION_ALREADY_WAITING.into());
                }
                // Record who clicked Cancel BEFORE the signal so the interrupt
                // arm, or the canceled startup, can stamp `ResponseCanceled.actor`
                // with the originating device (the popover's Device row).
                session.cancel_actor = actor;
                session.interrupt_or_cancel_startup();
                Ok(true)
            } else {
                drop(guard);
                if self.cancel_thread(tid, actor.clone()) {
                    return Ok(true);
                }
                let terminal = match terminal {
                    SettleTerminal::StuckProjection => SettleTerminal::UserStop,
                    terminal => terminal,
                };
                let settled = self
                    .settle_unclaimed_running_thread(tid, actor.clone(), terminal)
                    .await?;
                // No live session will settle what the stopped turn left, so
                // this Stop withholds it (ADR 0400).
                if settled && terminal == SettleTerminal::CanceledQuestion {
                    self.withhold_stopped_work(tid, actor).await;
                }
                Ok(settled)
            }
        } else {
            if guard.is_empty() {
                return Err("No Claude Code process is running".into());
            }
            let mut any = false;
            for session in guard.values() {
                if interrupt_route(session) == InterruptRoute::Interrupt {
                    session.interrupt_or_cancel_startup();
                    any = true;
                }
            }
            Ok(any)
        }
    }

    /// End the running turn on `thread_id` because a Hold reached it. The
    /// turn ends as `CancelCause::Hold`, which leaves no Stop hold, and the
    /// session stays resumable. Unlike Stop, it never settles a stale
    /// projection: that would write a Stop the Resume could not lift.
    /// `true` when a live turn or loop took the cancel.
    async fn stop_turn_for_hold(&self, thread_id: Uuid, actor: Option<MessageOrigin>) -> bool {
        let mut guard = self.agent_sessions.lock().await;
        if let Some(session) = guard
            .get_mut(&thread_id)
            .filter(|s| interrupt_route(s) == InterruptRoute::Interrupt)
        {
            session.cancel_actor = actor;
            session.redirect_cause = Some(crate::engine::thread_events::CancelCause::Hold);
            session.interrupt_or_cancel_startup();
            return true;
        }
        drop(guard);
        self.cancel_thread_for_hold(thread_id, actor)
    }

    /// Record a rung pick on a started thread, unless a turn runs
    /// ([`rung_locked`]). A finished turn's process gets [`IDLE_EXIT_WAIT`] to
    /// exit first. The pick is a `CodingAgentSettingsChanged` with the user's
    /// actor, which the next spawn reads through [`Self::cc_thread_settings`].
    /// No running process ever receives it.
    async fn record_rung_pick(
        &self,
        thread_id: Uuid,
        pick: RungPick,
        actor: Option<MessageOrigin>,
    ) -> Result<RungPickOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let coding_agent = self.thread_coding_agent(thread_id).await;
        let deadline = tokio::time::Instant::now() + IDLE_EXIT_WAIT;
        let sessions = loop {
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
                    .bind(thread_id)
                    .fetch_optional(self.pool())
                    .await?;
            let status = crate::engine::thread_lifecycle::ThreadStatus::parse(
                status.as_deref().unwrap_or_default(),
            );
            let sessions = self.agent_sessions.lock().await;
            let exiting = sessions
                .get(&thread_id)
                .filter(|s| awaits_idle_exit(s))
                .map(|s| s.idle_notify.clone());
            // The status still reads running between the turn's Result and its
            // idle event, so only a question park or a pause refuses at once.
            let parked = matches!(
                status,
                crate::engine::thread_lifecycle::ThreadStatus::WaitingForUserAnswer
                    | crate::engine::thread_lifecycle::ThreadStatus::Paused
            );
            match exiting {
                Some(idle_notify) if !parked && tokio::time::Instant::now() < deadline => {
                    // Woken or timed out alike, the next pass re-reads both.
                    let woken = idle_notify.notified();
                    drop(sessions);
                    _ = tokio::time::timeout(IDLE_EXIT_POLL, woken).await;
                }
                _ if rung_locked(status, sessions.get(&thread_id)) => {
                    return Ok(RungPickOutcome::TurnRunning);
                }
                _ => break sessions,
            }
        };
        let (model, reasoning_effort) = match pick {
            RungPick::SetModel { model, effort } => (Some(model), effort),
            RungPick::SetReasoningEffort { effort } => (None, Some(effort)),
        };
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::CodingAgentSettingsChanged {
                    model,
                    reasoning_effort,
                    coding_agent,
                    // A settings-only emit: the Init-time emit pins the
                    // session id and config dir.
                    cc_session_id: None,
                    claude_config_dir: None,
                    claude_config_dir_explicit: None,
                },
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                    actor,
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await?;
        drop(sessions);
        Ok(RungPickOutcome::Recorded)
    }

    async fn cc_categorized_commands(&self, thread_id: Uuid) -> CcCommandsResult {
        let guard = self.agent_sessions.lock().await;
        if let Some(s) = guard.get(&thread_id) {
            let has_active = s.is_live();
            let mut info = s.to_commands_info();
            // Only a live real turn reports its own pair. After a keep-alive
            // or an exit, a pick made since is what the next turn runs.
            let session_pair = (has_active && !s.keep_alive)
                .then(|| (s.current_model.clone(), s.current_reasoning_effort.clone()));
            let repo_key = s
                .repo_root
                .as_ref()
                .map(|r| r.to_string_lossy().to_string());
            drop(guard);
            // Session exists but Init hasn't arrived yet — use repo cache for commands
            if info.builtin_commands.is_empty() && info.skill_commands.is_empty() {
                if let Some(repo_key) = repo_key {
                    let cache = self.cc_commands_cache.read().await;
                    if let Some(cached) = cache.get(&repo_key) {
                        info.builtin_commands = cached.builtin_commands.clone();
                        info.skill_commands = cached.skill_commands.clone();
                    }
                }
            }
            let (model, effort) = match session_pair {
                Some(pair) => pair,
                None => self.cc_thread_settings(thread_id).await,
            };
            return CcCommandsResult {
                info,
                has_active_session: has_active,
                current_model: model,
                current_reasoning_effort: effort,
            };
        }
        drop(guard);
        // No live session — get commands from repo cache, settings from thread events
        let repo_root: Option<String> = sqlx::query_scalar(
            "SELECT repo_root FROM changes WHERE thread_id = $1 \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await
        .map_err(|e| {
            log_legacy!(
                "[ClaudeCode] Failed to look up repo_root for {}: {}",
                thread_id,
                e
            );
            e
        })
        .ok()
        .flatten();
        let info = if let Some(repo_key) = repo_root {
            let cache = self.cc_commands_cache.read().await;
            lookup_repo_commands_in_cache(&cache, &repo_key)
        } else {
            // Thread has no recorded repo (no `changes` row yet) — return
            // empty rather than leaking another repo's skills.
            CcCommandsInfo::default()
        };
        let (model, effort) = self.cc_thread_settings(thread_id).await;
        CcCommandsResult {
            info,
            has_active_session: false,
            current_model: model,
            current_reasoning_effort: effort,
        }
    }

    /// Return cached commands for a specific repo path (for compose-view menu).
    /// Empty result if the repo has never had a Claude Code session — never falls back
    /// to another repo's cache.
    async fn cc_commands_for_repo(&self, repo_path: &Path) -> CcCommandsResult {
        let cache = self.cc_commands_cache.read().await;
        let info = lookup_repo_commands_in_cache(&cache, &repo_path.to_string_lossy());
        CcCommandsResult {
            info,
            has_active_session: false,
            current_model: None,
            current_reasoning_effort: None,
        }
    }
}
