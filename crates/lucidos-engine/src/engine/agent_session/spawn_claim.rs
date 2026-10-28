//! Claim a thread for one coding-agent run.
//!
//! The claim inserts the thread's `agent_sessions` entry in the lock hold that
//! found the thread free. So the check and the claim are one step, and a second
//! spawn always sees the first. The entry starts in [`SessionPhase::Starting`]
//! and owns the message channel from the start. A follow-up sent during startup
//! queues for the run, and a Stop lands.
//!
//! A thread stays held until its run lets go of the receiver, not only until
//! the run marks its process exited. `finalize_direct_agent` still commits,
//! proposes or discards in the worktree after that mark, and nothing may spawn
//! into the worktree under it. A Tier-2 apply holds the thread the same way,
//! in `SessionPhase::Applying`, while it resets the worktree.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64};
use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::agent_session::child_reports::{child_reports_channel, ChildReportsLoopEnd};
use crate::engine::agent_session::withdraw::WithdrawInputRequest;
use crate::engine::types::{AgentSession, AgentUserInput, SessionPhase};
use crate::runtime::CodingAgent;

/// Why a claim did not take the thread.
#[derive(Debug)]
pub(crate) enum ClaimRefused {
    /// A run holds the thread and listens for input. The caller may route its
    /// input to that run instead.
    LiveSession,
    /// A run that is tearing down, or an apply, holds the thread. Wait until
    /// `holder.closed()` resolves (it let go) or `phase_changed` fires (an
    /// apply handed the thread to its merge session), then claim again.
    Held {
        holder: UnboundedSender<AgentUserInput>,
        phase_changed: Arc<Notify>,
    },
}

/// What a claim inserts the entry for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimFor {
    /// A coding-agent run: the entry starts `Starting`.
    Run,
    /// A Tier-2 apply: the entry is `Applying` until the apply lets go or
    /// hands the thread to its merge session.
    Apply,
    /// A *cache keep-alive* run. It starts `Starting` like a run, never waits
    /// on a hold, and holds the thread against real input until it ends.
    KeepAlive,
}

/// What the claimed entry shares with the run that claimed it.
pub(crate) struct ClaimedEntry {
    pub(crate) coding_agent: CodingAgent,
    /// Identity of the entry: `same_channel` tells it from a replacement.
    pub(crate) msg_tx: UnboundedSender<AgentUserInput>,
    pub(crate) msg_rx: UnboundedReceiver<AgentUserInput>,
    pub(crate) withdraw_rx: UnboundedReceiver<WithdrawInputRequest>,
    pub(crate) child_reports: ChildReportsLoopEnd,
    pub(crate) stop: Arc<Notify>,
    pub(crate) interrupt: Arc<Notify>,
    pub(crate) idle_notify: Arc<Notify>,
    pub(crate) startup_cancel: CancellationToken,
    pub(crate) agent_cancel: CancellationToken,
    pub(crate) shutting_down: Arc<AtomicBool>,
    pub(crate) external_terminal_emitted: Arc<AtomicBool>,
    pub(crate) external_continuation_requested: Arc<AtomicBool>,
    pub(crate) tools_in_flight: Arc<AtomicI32>,
    pub(crate) last_event_at: Arc<AtomicI64>,
    pub(crate) question_flushes: Arc<crate::engine::question_card_gate::QuestionFlushes>,
}

/// Whether `session`'s run still holds its thread. The receiver lives exactly
/// as long as the run does, so this needs no cleanup to have run.
pub(crate) fn holds_thread(session: &AgentSession) -> bool {
    !session.msg_tx.is_closed()
}

/// Claim `thread_id` under the caller's `agent_sessions` lock, or say who
/// holds it. An entry whose run has let go is replaced. Its model, effort and
/// command lists carry over, so the picker reads them until the next spawn
/// resolves its own rung.
pub(crate) fn claim_thread_for(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    coding_agent: CodingAgent,
    purpose: ClaimFor,
) -> Result<ClaimedEntry, ClaimRefused> {
    let applying = |s: &AgentSession| matches!(s.phase, SessionPhase::Applying);
    let previous = match sessions.get(&thread_id) {
        Some(s) if s.is_live() && !applying(s) && !s.keep_alive => {
            return Err(ClaimRefused::LiveSession)
        }
        Some(s) if holds_thread(s) => {
            return Err(ClaimRefused::Held {
                holder: s.msg_tx.clone(),
                phase_changed: s.idle_notify.clone(),
            })
        }
        previous => previous,
    };
    let (builtin_commands, skill_commands, current_model, current_reasoning_effort) = previous
        .map(|s| {
            (
                s.builtin_commands.clone(),
                s.skill_commands.clone(),
                s.current_model.clone(),
                s.current_reasoning_effort.clone(),
            )
        })
        .unwrap_or_default();

    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let (withdraw_tx, withdraw_rx) = tokio::sync::mpsc::unbounded_channel();
    let (child_reports, child_reports_loop) = child_reports_channel();
    let entry = ClaimedEntry {
        coding_agent,
        msg_tx,
        msg_rx,
        withdraw_rx,
        child_reports: child_reports_loop,
        stop: Arc::new(Notify::new()),
        interrupt: Arc::new(Notify::new()),
        idle_notify: Arc::new(Notify::new()),
        startup_cancel: CancellationToken::new(),
        agent_cancel: CancellationToken::new(),
        shutting_down: Arc::new(AtomicBool::new(false)),
        external_terminal_emitted: Arc::new(AtomicBool::new(false)),
        external_continuation_requested: Arc::new(AtomicBool::new(false)),
        tools_in_flight: Arc::new(AtomicI32::new(0)),
        last_event_at: Arc::new(AtomicI64::new(0)),
        question_flushes: Default::default(),
    };
    sessions.insert(
        thread_id,
        AgentSession {
            msg_tx: entry.msg_tx.clone(),
            is_waiting: false,
            pending_stop: None,
            cancel_actor: None,
            redirect_cause: None,
            redirect_followup_pending: false,
            stop: entry.stop.clone(),
            interrupt: entry.interrupt.clone(),
            idle_notify: entry.idle_notify.clone(),
            change_claim: None,
            conflict: None,
            process_exited: false,
            worktree_path: None,
            branch_name: None,
            repo_root: None,
            backend_session_id: None,
            last_event_at: entry.last_event_at.clone(),
            shutting_down: entry.shutting_down.clone(),
            external_terminal_emitted: entry.external_terminal_emitted.clone(),
            external_continuation_requested: entry.external_continuation_requested.clone(),
            phase: match purpose {
                ClaimFor::Run | ClaimFor::KeepAlive => SessionPhase::Starting {
                    startup_cancel: entry.startup_cancel.clone(),
                },
                ClaimFor::Apply => SessionPhase::Applying,
            },
            withdraw_tx,
            child_reports,
            unforwarded_inputs: Default::default(),
            builtin_commands,
            skill_commands,
            current_model,
            current_reasoning_effort,
            reasoning_effort_in_force: None,
            question_resume_pending: false,
            quiet_answer_pending: None,
            tools_in_flight: entry.tools_in_flight.clone(),
            question_flushes: entry.question_flushes.clone(),
            coding_agent,
            agent_cancel: entry.agent_cancel.clone(),
            keep_alive: purpose == ClaimFor::KeepAlive,
        },
    );
    Ok(entry)
}

/// Flip an apply's `Applying` entry to `Starting`, under the caller's lock,
/// and wake the spawns waiting on it.
pub(crate) fn hand_hold_to_run(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    hold: &ClaimedEntry,
) {
    if let Some(session) = sessions
        .get_mut(&thread_id)
        .filter(|s| s.msg_tx.same_channel(&hold.msg_tx))
    {
        session.phase = SessionPhase::Starting {
            startup_cancel: hold.startup_cancel.clone(),
        };
        session.idle_notify.notify_waiters();
    }
}

/// Put the model and effort a request picked on its claimed entry, under the
/// caller's lock. The commands API reads the entry from the claim on, so the
/// pick shows while the spawn is still starting. A pick the request left out
/// keeps the claim's carry-over.
pub(crate) fn name_requested_settings(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    claimed: &ClaimedEntry,
    model: Option<&str>,
    effort: Option<&str>,
) {
    let Some(session) = sessions
        .get_mut(&thread_id)
        .filter(|s| s.msg_tx.same_channel(&claimed.msg_tx))
    else {
        return;
    };
    if let Some(model) = model {
        session.current_model = Some(model.to_string());
    }
    if let Some(effort) = effort {
        session.current_reasoning_effort = Some(effort.to_string());
    }
}

/// A thread claimed for one run. `run_direct_agent` takes it by value, so no
/// spawn can start without one.
pub(crate) struct SpawnClaim {
    pub(crate) thread_id: Uuid,
    pub(crate) entry: ClaimedEntry,
    /// Reaps the entry when the claim is dropped before its run lets go.
    pub(in crate::engine::agent_session) entry_guard:
        super::run_session::entry_guard::SessionEntryGuard,
    /// Re-submits the follow-ups the run never read. Weak, because the claim
    /// lives inside a run future the engine outlives.
    engine: std::sync::Weak<crate::engine::LucidosEngine>,
}

impl crate::engine::LucidosEngine {
    /// Claim `thread_id` for a coding-agent run. `None` means a live run holds
    /// the thread, so the caller routes to it or refuses. A run that is tearing
    /// down is waited out, because its worktree work is not done yet.
    pub(crate) async fn claim_thread_for_spawn(
        &self,
        thread_id: Uuid,
        requested_coding_agent: Option<CodingAgent>,
    ) -> Result<Option<SpawnClaim>, Box<dyn std::error::Error + Send + Sync>> {
        self.claim_thread_waiting(thread_id, requested_coding_agent, ClaimFor::Run)
            .await
    }

    /// Claim for a *cache keep-alive*. `None` when a live run or a hold has
    /// the thread: a keep-alive skips its beat rather than wait on either.
    pub(crate) async fn claim_thread_for_keep_alive(
        &self,
        thread_id: Uuid,
    ) -> Result<Option<SpawnClaim>, Box<dyn std::error::Error + Send + Sync>> {
        self.claim_thread_waiting(thread_id, None, ClaimFor::KeepAlive)
            .await
    }

    /// [`name_requested_settings`] for `claim`, taking the lock.
    pub(crate) async fn name_requested_settings(
        &self,
        claim: &SpawnClaim,
        model: Option<&str>,
        effort: Option<&str>,
    ) {
        name_requested_settings(
            &mut *self.agent_sessions.lock().await,
            claim.thread_id,
            &claim.entry,
            model,
            effort,
        );
    }

    /// Turn an apply's hold into the claim of the merge session it hands the
    /// thread to, in one lock hold. Spawns waiting on the hold wake and find a
    /// live run, so a follow-up routes to the merge session.
    pub(crate) async fn hand_apply_hold_to_run(&self, hold: SpawnClaim) -> SpawnClaim {
        hand_hold_to_run(
            &mut *self.agent_sessions.lock().await,
            hold.thread_id,
            &hold.entry,
        );
        hold
    }

    /// Hold `thread_id` while a Tier-2 apply or a late Discard resets its
    /// worktree. `None` means a live run holds the thread, whose worktree must
    /// not be reset.
    pub(crate) async fn hold_thread_for_worktree_reset(
        &self,
        thread_id: Uuid,
    ) -> Result<Option<SpawnClaim>, Box<dyn std::error::Error + Send + Sync>> {
        self.claim_thread_waiting(thread_id, None, ClaimFor::Apply)
            .await
    }

    async fn claim_thread_waiting(
        &self,
        thread_id: Uuid,
        requested_coding_agent: Option<CodingAgent>,
        purpose: ClaimFor,
    ) -> Result<Option<SpawnClaim>, Box<dyn std::error::Error + Send + Sync>> {
        // Read only when the thread is free: a follow-up that routes to a live
        // run, or waits on one, never needs the backend.
        let mut coding_agent = None;
        loop {
            let mut sessions = self.agent_sessions.lock().await;
            let free = sessions.get(&thread_id).is_none_or(|s| !holds_thread(s));
            let agent = match coding_agent {
                Some(agent) => agent,
                None if free => {
                    drop(sessions);
                    coding_agent = Some(
                        resolve_thread_coding_agent(self.pool(), thread_id, requested_coding_agent)
                            .await?,
                    );
                    continue;
                }
                // Not used: a held thread refuses before the insert.
                None => CodingAgent::ClaudeCode,
            };
            let claimed = claim_thread_for(&mut sessions, thread_id, agent, purpose);
            match claimed {
                Ok(entry) => {
                    let entry_guard = super::run_session::entry_guard::SessionEntryGuard::new(
                        self.agent_sessions.clone(),
                        thread_id,
                        entry.msg_tx.clone(),
                        entry.shutting_down.clone(),
                        self.event_bus.clone(),
                        self.pool.clone(),
                    );
                    return Ok(Some(SpawnClaim {
                        thread_id,
                        entry,
                        entry_guard,
                        engine: self
                            .try_clone_arc()
                            .map(|engine| Arc::downgrade(&engine))
                            .unwrap_or_default(),
                    }));
                }
                Err(ClaimRefused::LiveSession) => return Ok(None),
                Err(ClaimRefused::Held { .. }) if purpose == ClaimFor::KeepAlive => {
                    return Ok(None)
                }
                Err(ClaimRefused::Held {
                    holder,
                    phase_changed,
                }) => {
                    // Registered under the lock, so a hand-off cannot slip by.
                    let handed_off = phase_changed.notified_owned();
                    drop(sessions);
                    tokio::select! {
                        _ = holder.closed() => {}
                        _ = handed_off => {}
                    }
                }
            }
        }
    }
}

impl crate::engine::LucidosEngine {
    /// Claim for an engine-made run (a merge, hardening), which has no input to
    /// route to a live run. A live run on the thread refuses it.
    pub(crate) async fn claim_thread_or_refuse(
        &self,
        thread_id: Uuid,
    ) -> Result<SpawnClaim, Box<dyn std::error::Error + Send + Sync>> {
        self.claim_thread_for_spawn(thread_id, None)
            .await?
            .ok_or_else(|| crate::engine::claude_code::AGENT_ALREADY_RUNNING_ERROR.into())
    }

    /// Settle a `running` projection that no run holds. The settle holds the
    /// claim, so no spawn can start between its check and its terminal. A run
    /// that holds the thread owns the turn, and nothing is settled.
    pub(crate) async fn settle_unclaimed_running_thread(
        &self,
        thread_id: Uuid,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
        terminal: crate::engine::claude_code::SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let Some(_claim) = self.claim_thread_for_spawn(thread_id, None).await? else {
            return Ok(false);
        };
        crate::engine::claude_code::settle_stuck_running_thread(
            &self.pool,
            &self.event_bus,
            thread_id,
            actor,
            terminal,
        )
        .await
    }

    /// Queue `input` for the live run that holds `thread_id`. It reads it as
    /// a follow-up: now when idle, after its first prompt when starting, and
    /// at its next turn boundary when mid-turn. `false` when no run takes it.
    pub(crate) async fn route_to_live_run(&self, thread_id: Uuid, input: AgentUserInput) -> bool {
        let sessions = self.agent_sessions.lock().await;
        sessions
            .get(&thread_id)
            .filter(|s| s.is_live() && !matches!(s.phase, SessionPhase::Applying))
            .is_some_and(|s| s.msg_tx.send(input).is_ok())
    }

    /// End the turn a Stop cancelled during startup. The shutdown sweep owns
    /// the terminal of a teardown, so a shutdown emits nothing here.
    pub(in crate::engine::agent_session) async fn settle_canceled_startup(
        &self,
        claim: &SpawnClaim,
        request_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
    ) -> crate::engine::types::ProcessResult {
        let thread_id = claim.thread_id;
        log!(
            "[AgentSession] A Stop cancelled the spawn for thread {} before its agent took a turn",
            thread_id
        );
        let shutting_down = claim
            .entry
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire);
        if !self.session_is_shutting_down(shutting_down) {
            let mut meta = meta.clone();
            if let Some(actor) = self.take_session_cancel_actor(thread_id).await {
                meta.actor = Some(actor);
            }
            crate::engine::thread_events::emit_response_canceled(
                &self.event_bus,
                &self.pool,
                thread_id,
                crate::engine::thread_events::CancelCause::UserStop,
                String::new(),
                vec![],
                None,
                None,
                meta,
                "[AgentSession] ResponseCanceled (stopped during startup)",
            )
            .await;
        }
        crate::engine::types::ProcessResult {
            response: String::new(),
            steps: vec![],
            images: vec![],
            request_id,
            thread_id,
            proposed_change: false,
            auto_apply: false,
            orphaned_injections: vec![],
        }
    }
}

impl Drop for SpawnClaim {
    /// A follow-up can reach the claim while its run starts, and the run can
    /// end before it reads it: an early error, a Stop, a stale resume. Every
    /// such exit hands the follow-up to the orphan chain, so it is answered.
    fn drop(&mut self) {
        // Closed first, so no send lands after the drain and is lost. A closed
        // receiver also reads as "let go" to every claim and fast path.
        self.entry.msg_rx.close();
        let leftovers = super::io_helpers::lost_followups_to_orphans(
            super::io_helpers::drain_lost_followups(&mut self.entry.msg_rx),
        );
        if leftovers.is_empty() {
            return;
        }
        match self.engine.upgrade() {
            Some(engine) => engine.resubmit_orphans_of_failed_turn(self.thread_id, leftovers),
            None => log!(
                "[AgentSession] Dropped {} follow-up(s) on thread {}: no engine to re-submit them",
                leftovers.len(),
                self.thread_id
            ),
        }
    }
}

/// The backend a thread runs on. Only a thread's FIRST session honours the
/// request. A *successor session* runs on the backend its handoff named.
/// Otherwise the stored `thread_summaries.coding_agent` wins: a new backend
/// would have no session to resume and would lose all context.
pub(crate) async fn resolve_thread_coding_agent(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    requested: Option<CodingAgent>,
) -> Result<CodingAgent, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(handoff) = crate::engine::successor::pending_handoff(pool, thread_id).await? {
        return Ok(handoff.coding_agent);
    }
    let stored: Option<String> =
        sqlx::query_scalar("SELECT coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(match stored {
        Some(s) => {
            let stored_agent = CodingAgent::parse(&s);
            if requested.is_some_and(|req| req != stored_agent) {
                log!(
                    "[AgentSession] thread {} is locked to {:?} — ignoring requested backend {:?}",
                    thread_id,
                    stored_agent,
                    requested
                );
            }
            stored_agent
        }
        None => requested.unwrap_or(CodingAgent::ClaudeCode),
    })
}

#[cfg(test)]
#[path = "spawn_claim_tests.rs"]
mod tests;
