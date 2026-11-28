//! INVARIANT: Claim a thread for one coding-agent run.
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

use crate::engine::child_report_channel::{child_reports_channel, ChildReportsLoopEnd};
use crate::engine::types::WithdrawInputRequest;
use crate::engine::types::{AgentSession, AgentUserInput, SessionPhase};
use crate::runtime::CodingAgent;

#[derive(Debug)]
pub enum ClaimRefused {
    LiveSession,
    Held {
        holder: UnboundedSender<AgentUserInput>,
        phase_changed: Arc<Notify>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimFor {
    Run,
    Apply,
    KeepAlive,
}

impl ClaimFor {
    pub fn settles_owed_handoff(self) -> bool {
        self == Self::Run
    }
}

pub struct ClaimedEntry {
    pub coding_agent: CodingAgent,
    pub msg_tx: UnboundedSender<AgentUserInput>,
    pub msg_rx: UnboundedReceiver<AgentUserInput>,
    pub withdraw_rx: UnboundedReceiver<WithdrawInputRequest>,
    pub child_reports: ChildReportsLoopEnd,
    pub stop: Arc<Notify>,
    pub interrupt: Arc<Notify>,
    pub idle_notify: Arc<Notify>,
    pub startup_cancel: CancellationToken,
    pub agent_cancel: CancellationToken,
    pub shutting_down: Arc<AtomicBool>,
    pub external_terminal_emitted: Arc<AtomicBool>,
    pub external_continuation_requested: Arc<AtomicBool>,
    pub tools_in_flight: Arc<AtomicI32>,
    pub(crate) last_event_at: Arc<AtomicI64>,
    pub question_flushes: Arc<crate::engine::question_flushes::QuestionFlushes>,
}

pub fn holds_thread(session: &AgentSession) -> bool {
    !session.msg_tx.is_closed()
}

pub fn claim_thread_for(
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
            sign_in_context: None,
            execution_sandbox: None,
            msg_tx: entry.msg_tx.clone(),
            is_waiting: false,
            pending_stop: None,
            cancel_actor: None,
            redirect_cause: None,
            redirect_followup_pending: false,
            respawn_after_sign_in: false,
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
            in_ceiling_reentry: false,
            tools_in_flight: entry.tools_in_flight.clone(),
            question_flushes: entry.question_flushes.clone(),
            coding_agent,
            can_steer: coding_agent == CodingAgent::ClaudeCode,
            agent_cancel: entry.agent_cancel.clone(),
            keep_alive: purpose == ClaimFor::KeepAlive,
        },
    );
    Ok(entry)
}

pub fn hand_hold_to_run(
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

pub fn name_requested_settings(
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

pub struct SpawnClaim {
    pub thread_id: Uuid,
    pub entry: ClaimedEntry,
    pub entry_guard: super::session_entry_guard::SessionEntryGuard,
    engine: std::sync::Weak<crate::engine::LucidosEngine>,
}

impl crate::engine::LucidosEngine {
    /// INVARIANT: Claim `thread_id` for a coding-agent run. `None` means a live run holds
    /// the thread, so the caller routes to it or refuses. A run that is tearing
    /// down is waited out, because its worktree work is not done yet.
    pub async fn claim_thread_for_spawn(
        &self,
        thread_id: Uuid,
        requested_coding_agent: Option<CodingAgent>,
    ) -> Result<Option<SpawnClaim>, Box<dyn std::error::Error + Send + Sync>> {
        self.claim_thread_waiting(thread_id, requested_coding_agent, ClaimFor::Run)
            .await
    }

    pub async fn claim_thread_for_keep_alive(
        &self,
        thread_id: Uuid,
    ) -> Result<Option<SpawnClaim>, Box<dyn std::error::Error + Send + Sync>> {
        self.claim_thread_waiting(thread_id, None, ClaimFor::KeepAlive)
            .await
    }

    pub async fn name_requested_settings(
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

    pub async fn hand_apply_hold_to_run(&self, hold: SpawnClaim) -> SpawnClaim {
        hand_hold_to_run(
            &mut *self.agent_sessions.lock().await,
            hold.thread_id,
            &hold.entry,
        );
        hold
    }

    pub async fn hold_thread_for_worktree_reset(
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
        let mut coding_agent = None;
        loop {
            let mut sessions = self.agent_sessions.lock().await;
            let free = sessions.get(&thread_id).is_none_or(|s| !holds_thread(s));
            let agent = match coding_agent {
                Some(agent) => agent,
                None if free => {
                    drop(sessions);
                    // INVARIANT: Before the backend: the handoff names the successor's.
                    if purpose.settles_owed_handoff() {
                        self.settle_owed_ceiling_handoff(thread_id).await;
                    }
                    coding_agent = Some(
                        resolve_thread_coding_agent(self.pool(), thread_id, requested_coding_agent)
                            .await?,
                    );
                    continue;
                }
                None => CodingAgent::ClaudeCode,
            };
            let claimed = claim_thread_for(&mut sessions, thread_id, agent, purpose);
            match claimed {
                Ok(entry) => {
                    let entry_guard = super::session_entry_guard::SessionEntryGuard::new(
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
                    // INVARIANT: Registered under the lock, so a hand-off cannot slip by.
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
    pub async fn claim_thread_or_refuse(
        &self,
        thread_id: Uuid,
    ) -> Result<SpawnClaim, Box<dyn std::error::Error + Send + Sync>> {
        self.claim_thread_for_spawn(thread_id, None)
            .await?
            .ok_or_else(|| crate::engine::continuation_reason::AGENT_ALREADY_RUNNING_ERROR.into())
    }

    pub async fn route_to_live_run(&self, thread_id: Uuid, input: AgentUserInput) -> bool {
        let sessions = self.agent_sessions.lock().await;
        sessions
            .get(&thread_id)
            .filter(|s| s.is_live() && !matches!(s.phase, SessionPhase::Applying))
            .is_some_and(|s| s.msg_tx.send(input).is_ok())
    }
}

impl Drop for SpawnClaim {
    fn drop(&mut self) {
        // INVARIANT: Closed first, so no send lands after the drain and is lost. A closed
        // receiver also reads as "let go" to every claim and fast path.
        self.entry.msg_rx.close();
        let leftovers = super::session_follow_ups::lost_followups_to_orphans(
            super::session_follow_ups::drain_lost_followups(&mut self.entry.msg_rx),
        );
        if leftovers.is_empty() {
            return;
        }
        match self.engine.upgrade() {
            Some(engine) => engine.resubmit_orphans_of_failed_turn(self.thread_id, leftovers),
            None => log_legacy!(
                "[AgentSession] Dropped {} follow-up(s) on thread {}: no engine to re-submit them",
                leftovers.len(),
                self.thread_id
            ),
        }
    }
}

/// INVARIANT: The backend a thread runs on. Only a thread's FIRST session honours the
/// request. A *successor session* runs on the backend its handoff named.
/// Otherwise the stored `thread_summaries.coding_agent` wins: a new backend
/// would have no session to resume and would lose all context.
pub async fn resolve_thread_coding_agent(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    requested: Option<CodingAgent>,
) -> Result<CodingAgent, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(handoff) = crate::engine::pending_handoff::pending_handoff(pool, thread_id).await? {
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
                log_legacy!(
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
