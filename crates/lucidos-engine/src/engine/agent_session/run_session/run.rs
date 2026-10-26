use super::completion::{idle_exit, IdleExit};
use super::idle_change_state::{resolve_idle_change_state, IdleChangeStateInput};
use super::idle_snapshot::CodingAgentIdleSnapshot;
use super::spawn_context::SpawnWorktreeContext;
use crate::engine::agent_context::{
    prepend_user_device_preferences_context, ContextFingerprint, DeviceBlock, SentDeviceContexts,
};
use crate::engine::agent_session::start_failure::AgentStartFailure;
use crate::engine::agent_session::text_buffer::CodingAgentTextBuffer;
use crate::engine::agentic_loop::should_flush;
use crate::engine::change_ops::{now_epoch_millis, ProposeOutcome};
use crate::engine::claude_code::STALE_RESUME_ERROR;
use crate::engine::git_ops::{
    auto_commit_preserving_marker, branch_head_sha, default_local_branch, describe_branch_changes,
    files_require_restart, has_full_harden_marker, is_external_repo_path, main_worktree,
};
use crate::engine::thread_events::{
    CodingAgentExit, CodingAgentExitStage, EventChannel, SessionEndReason,
};
use crate::engine::{AgentUserInput, LucidosEngine, ProcessResult, StopReason};
use crate::runtime::{AgentEvent, AgentExitFacts, AgentInput, CodingAgent};
use futures::StreamExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::engine::agent_session::child_reports::{begin_take, settle_take};
use crate::engine::agent_session::continuity::resolve_session_continuity;
use crate::engine::agent_session::input_ledger::{
    announce_reads, forwards_verbatim, InputLedger, Settled,
};
use crate::engine::agent_session::io_helpers::{
    drain_lost_followups, lost_followups_to_orphans, requeue_at_head,
};
use crate::engine::agent_session::lifecycle::{
    classify_result, idle_action, idle_change_write, is_definitive_session_not_found,
    is_resume_settle_result, is_stale_resume_signal, require_agent_input, reset_per_turn_flags,
    result_auto_commits, should_auto_commit_on_cleanup, starts_turn_after_terminal,
    terminal_clears_user_hit_stop, terminate_decision, turn_has_terminal, watchdog_gate,
    IdleAction, IdleChangeWrite, IdleSession, StaleResumeInputs, TerminalKind, TerminateDecision,
    TurnTerminal, WatchdogGate, WATCHDOG_DIAG_LOG_THRESHOLD_MS, WATCHDOG_HUNG_TOOL_CEILING_MS,
    WATCHDOG_INACTIVITY_LIMIT_MS, WATCHDOG_TICK_INTERVAL_SECS,
};
use crate::engine::agent_session::prompts::standing_instruction_section;
use crate::engine::agent_session::resume::{
    change_description_fallback, default_claude_config_dir, live_claude_config_dir,
    resolve_resume_context,
};
use crate::engine::agent_session::spawn::{spawn_or_resume, ResumeAttempt};
use crate::engine::agent_session::withdraw::{
    begin_withdraw, finish_withdraw, settle_answered_withdraws, PendingWithdraws,
    WithdrawInputRequest,
};
use crate::engine::chat::held_deliveries::anchor_is_child_report;
use crate::engine::question_card_gate::LoopEndsFlushes;

/// Decrement the paired-tool counter, flooring at 0. An unpaired decrement going
/// negative would permanently disarm the hang watchdog, since `watchdog_gate`
/// reads `tools_in_flight > 0` and would never gate-skip again. So every
/// decrement site shares this one contract. `Relaxed` matches the increments:
/// the only reader tolerates one-tick staleness.
fn release_tool_slot(tools_in_flight: &std::sync::atomic::AtomicI32) {
    let prev = tools_in_flight.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    if prev <= 0 {
        tools_in_flight.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Is a `Result` whose turn streamed no assistant text carrying the agent's own
/// prose, worth emitting as `CodingAgentTextStreamed`?
///
/// Yes for the case that branch exists for: a slash command answers through
/// `result.result` with no preceding Message. No when the text IS the turn's
/// failure reason. That string is about to be emitted as `ResponseFailed`, and
/// the failure card renders it. Printing it as a paragraph too states one
/// failure twice.
///
/// Deliberately an equality check rather than an `API Error` prefix test: it can
/// only ever drop text the card is already showing verbatim, so no genuine model
/// prose is at risk. A failure whose reason came from elsewhere leaves the text
/// alone and falls through to the frontend's own echo drop.
fn result_text_is_own_prose(text: &str, cc_error: Option<&str>) -> bool {
    let text = text.trim();
    !text.is_empty() && cc_error.map(str::trim) != Some(text)
}

/// Wait until the killed agent's process group is gone. The driver drops its
/// events sender only after the kill completes, so a closed channel is a
/// precise signal. Bounded, so a stuck teardown cannot wedge the caller.
async fn await_process_teardown(
    events_rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::runtime::AgentEvent>,
    thread_id: Uuid,
    why: &str,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match tokio::time::timeout_at(deadline, events_rx.recv()).await {
            // Trailing events from the dying process: drop them.
            Ok(Some(_)) => continue,
            Ok(None) => return,
            Err(_) => {
                log!(
                    "[AgentSession] teardown wait after {} timed out for thread {}; the old process may briefly linger",
                    why,
                    thread_id
                );
                return;
            }
        }
    }
}

/// How a driver that wound down before the first prompt could be sent ended.
#[derive(Debug, Default)]
struct StartupDeath {
    /// The cause to show: the driver's own `Result` error, else the CLI's
    /// stderr, else a note that a signal killed it.
    reason: Option<String>,
    exit: AgentExitFacts,
}

/// The coding-agent driver dropped its input receiver before the engine could
/// send the first prompt. That happens only when the driver task already wound
/// down: the agent process failed to start, exited immediately, or its handshake
/// failed. In every such path the driver flushes its REAL cause onto `events_rx`
/// first, as a `Result { error }` or an `Exited`. Recover it, so the thread
/// surfaces an actionable reason.
///
/// Non-blocking on purpose. By the time the receiver-drop is observed the driver
/// has returned, so every event it will ever emit is already buffered.
/// `try_recv` drains the tail without awaiting.
fn drain_startup_death(
    events_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
) -> StartupDeath {
    let mut error: Option<String> = None;
    let mut exit = AgentExitFacts::default();
    while let Ok(ev) = events_rx.try_recv() {
        match ev {
            // Last non-empty error wins: the driver emits the specific cause
            // before the terminal Exited.
            AgentEvent::Result { error: Some(e), .. } if !e.trim().is_empty() => {
                error = Some(e);
            }
            AgentEvent::Exited { exit: facts } => exit = facts,
            _ => {}
        }
    }
    let reason = error
        .or_else(|| (!exit.stderr_tail.is_empty()).then(|| exit.stderr_tail.clone()))
        .or_else(|| {
            exit.killed_by_signal
                .then(|| "coding agent process was killed by a signal during startup".to_string())
        });
    StartupDeath { reason, exit }
}

#[derive(Debug, PartialEq, Eq)]
enum StartupFailure {
    /// Claude Code refused the `--resume` because the session is gone. The run
    /// loop recovers the same refusal when it lands after the first prompt, so
    /// both routes end in one fresh-session retry.
    StaleResume,
    Failed(String),
}

fn classify_startup_failure(resumed: bool, cause: Option<String>) -> StartupFailure {
    match cause {
        Some(c) if resumed && is_definitive_session_not_found(Some(&c)) => {
            StartupFailure::StaleResume
        }
        Some(c) => StartupFailure::Failed(format!(
            "Coding agent exited during startup before the first prompt could be sent: {c}"
        )),
        None => StartupFailure::Failed(
            "Coding agent exited during startup before the first prompt could be sent".to_string(),
        ),
    }
}

/// The error a start failure returns once the first prompt could not be sent.
///
/// A death that left facts becomes an [`AgentStartFailure`] carrying the CLI's
/// own words, so the turn fails with its stage and exit code. One that left
/// nothing (a driver that panicked) keeps the plain message.
fn startup_failure_error(
    coding_agent: CodingAgent,
    death: StartupDeath,
    plain_message: String,
) -> Box<dyn std::error::Error + Send + Sync> {
    let StartupDeath { reason, exit } = death;
    if reason.is_none() && exit.code.is_none() {
        return plain_message.into();
    }
    let stage = CodingAgentExitStage::Startup;
    Box::new(AgentStartFailure {
        message: reason.unwrap_or_else(|| {
            crate::engine::agent_session::lifecycle::exit_error_text(coding_agent, stage, &exit)
        }),
        exit: CodingAgentExit {
            stage,
            code: exit.code,
        },
    })
}

/// Where a Lucidos-**source** session roots when no `Lucidos` repository row
/// resolved and the spawn isn't an app spawn.
///
/// `dev_root` comes from [`main_worktree`], which falls back to the process cwd
/// when there is no source checkout. On a packaged install that is the
/// **workspace** directory, itself a git repo. Returning it there would branch
/// the user's workspace git and label it platform source.
///
/// So the fallback survives only for its real case, a dev build whose registry
/// row has not been written yet. Everything else is a hard refusal. It backstops
/// every caller; the `run_coding_agent` tool refuses the same case earlier, so
/// the model sees it in-turn.
///
/// Pure over `has_lucidos_source`, so both branches are testable without a
/// packaged binary. See `docs/plans/2026-07-29-no-lucidos-source-agent-context.md`.
fn unregistered_lucidos_root(
    dev_root: PathBuf,
    has_lucidos_source: bool,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if has_lucidos_source {
        return Ok(dev_root);
    }
    Err(crate::engine::agent_session::err_no_lucidos_source())
}

/// The token counts of one `AgentEvent::Usage`, as the agent reported them.
#[derive(Debug)]
struct ReportedUsage {
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    cache_creation_1h_tokens: Option<u32>,
    parent_tool_use_id: Option<String>,
    api_call_id: Option<String>,
    duration_ms: Option<u64>,
    reasoning_effort: Option<String>,
}

/// How long a stopped session waits for Claude Code's driver to flush a
/// cut-off call. The flush follows the stop at once, ahead of the group
/// teardown, so this bounds only a driver that is stuck.
const USAGE_FLUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// The `Usage` events a stopped driver sends, read up to its `OutputEnded`
/// (or `Exited`, or a closed channel) and at most for `wait`. Every other
/// event is dropped, as the stopped loop would have dropped it.
async fn usage_flushed_at_stop(
    events_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    wait: std::time::Duration,
) -> Vec<(Option<String>, ReportedUsage)> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut flushed = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, events_rx.recv()).await {
            Ok(Some(AgentEvent::Usage {
                model,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                cache_creation_1h_tokens,
                parent_tool_use_id,
                api_call_id,
                duration_ms,
                reasoning_effort,
            })) => flushed.push((
                model,
                ReportedUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens,
                    cache_creation_1h_tokens,
                    parent_tool_use_id,
                    api_call_id,
                    duration_ms,
                    reasoning_effort,
                },
            )),
            Ok(Some(AgentEvent::OutputEnded | AgentEvent::Exited { .. })) | Ok(None) => break,
            Ok(Some(_)) => {}
            Err(_) => {
                log!(
                    "[AgentSession] no OutputEnded within {}s of the stop; a cut-off call may go unrecorded",
                    wait.as_secs()
                );
                break;
            }
        }
    }
    flushed
}

/// The two model ids one coding-agent call records on its `ContextCaptured`.
#[derive(Debug, PartialEq)]
struct CallModels {
    /// The id the reply named, never the picker alias it folds onto, so a
    /// reader can price it. The agent strips the session's `[1m]` from its
    /// echo, and it comes back here, since the context window keys on it.
    model: String,
    /// The *served model*: the id the reply named, verbatim. It always reads
    /// as served as sent beside `model`, since one reply names both.
    served_model: Option<String>,
}

impl CallModels {
    /// `session_model` is the model the session was started on, and
    /// `reported_model` the one the agent echoed for this call. A sub-agent's
    /// call echoes its own model, never the session's.
    fn of(session_model: Option<&str>, reported_model: Option<&str>) -> Self {
        let reported_model = reported_model.filter(|m| !m.is_empty());
        let with_session_window = |reported: &str| {
            let reconciled =
                crate::runtime::claude_code::reconcile_cc_model(session_model, reported);
            match reconciled.ends_with("[1m]") {
                true => format!("{reported}[1m]"),
                false => reported.to_string(),
            }
        };
        Self {
            model: reported_model
                .map(with_session_window)
                .or_else(|| session_model.map(String::from))
                .unwrap_or_default(),
            served_model: reported_model.map(String::from),
        }
    }
}

/// The reasoning effort a coding-agent call ran at. `spawn_effort` is the one
/// this subprocess was started with, and `reported_effort` the one the driver
/// sent with the call's turn.
fn effort_in_force(
    coding_agent: CodingAgent,
    spawn_effort: Option<&str>,
    reported_effort: Option<String>,
) -> Option<String> {
    match coding_agent {
        // Claude Code takes effort only at spawn, as `CLAUDE_CODE_EFFORT_LEVEL`.
        CodingAgent::ClaudeCode => spawn_effort.map(String::from),
        // Codex reports the effort it validated for each turn.
        CodingAgent::Codex => reported_effort,
    }
}

/// The `ContextCaptured` that records one coding-agent API call. Sections
/// stay empty, because the agent does not expose its system prompt or tool
/// schemas.
fn coding_agent_capture(
    coding_agent: CodingAgent,
    models: CallModels,
    context_window: usize,
    reported: ReportedUsage,
    reasoning_effort: Option<String>,
) -> crate::engine::thread_events::ThreadEvent {
    // Anthropic reports `input_tokens` as the uncached portion only.
    // `ApiUsage.input_tokens` stores the TOTAL prompt size, the same
    // convention `vertex.rs` uses. So the budget bar shows real context
    // use, and the modal's cache-miss formula recovers the uncached count.
    let total_input = reported
        .input_tokens
        .saturating_add(reported.cache_read_tokens)
        .saturating_add(reported.cache_creation_tokens);
    let estimated_total_tokens = (total_input as usize) + (reported.output_tokens as usize);
    let usage = crate::engine::ApiUsage {
        input_tokens: total_input,
        output_tokens: reported.output_tokens,
        cache_read_tokens: reported.cache_read_tokens,
        cache_creation_tokens: reported.cache_creation_tokens,
        // A coding agent reports one blended total per direction.
        modality: None,
        service_tier: None,
        cache_creation_1h_tokens: reported.cache_creation_1h_tokens,
        cache_creation_30m_tokens: None,
        cache_creation_12h_tokens: None,
        cache_creation_requested_ttl: None,
    };
    crate::engine::thread_events::ThreadEvent::ContextCaptured {
        producer: crate::engine::ContextProducer::from_coding_agent(coding_agent),
        model: models.model,
        context_window,
        sections: Vec::new(),
        tools: Vec::new(),
        estimated_total_tokens,
        usage: Some(usage),
        trimmed: false,
        // This path never runs the trimmer, so no pass can have fired.
        trim_passes: Vec::new(),
        purpose: crate::engine::ContextPurpose::Turn,
        reconstructed: false,
        parent_tool_use_id: reported.parent_tool_use_id,
        api_call_id: reported.api_call_id,
        reasoning_effort,
        duration_ms: reported.duration_ms,
        served_model: models.served_model,
        provider: None,
        stable_prefix_fingerprint: None,
        ttft_ms: None,
        request_id: None,
        session: None,
    }
}

impl LucidosEngine {
    /// Record one coding-agent API call as a `ContextCaptured` carrying its
    /// usage and the effort it ran at. A main-session call also sets the
    /// session's effort in force, which `token` names as this run's.
    #[allow(clippy::too_many_arguments)]
    async fn record_coding_agent_usage(
        &self,
        thread_id: Uuid,
        token: &std::sync::atomic::AtomicBool,
        meta: &crate::engine::thread_events::EventMeta,
        coding_agent: CodingAgent,
        models: CallModels,
        mut reported: ReportedUsage,
        spawn_effort: Option<&str>,
    ) {
        let reasoning_effort =
            effort_in_force(coding_agent, spawn_effort, reported.reasoning_effort.take());
        if reported.parent_tool_use_id.is_none() {
            super::completion::note_effort_in_force(
                &mut *self.agent_sessions.lock().await,
                thread_id,
                token,
                reasoning_effort.as_deref(),
            );
        }
        // The backend's own window wins, because this capture REPORTS a call
        // the engine did not make. `context_window_for` answers for a Lucidos
        // request, where 1M mode is gated on our `[1m]` suffix. A coding agent
        // picks its own mode, so a bare Sonnet 5 id really does run 1M there.
        let context_window =
            crate::runtime::coding_agent_context_window(coding_agent, &models.model)
                .unwrap_or_else(|| self.context_window_for(&models.model, None));
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: coding_agent_capture(
                        coding_agent,
                        models,
                        context_window,
                        reported,
                        reasoning_effort,
                    ),
                    meta: meta.clone(),
                },
                "[AgentSession] ContextCaptured",
            )
            .await;
    }

    /// Decide the idle exit again after the ledger changed outside an agent
    /// event. A withdraw or a settled take can leave an idle session owing
    /// nothing, and only an idle decides the exit. `true` when the session
    /// should end.
    async fn idle_exit_ends_session(
        &self,
        thread_id: Uuid,
        msg_rx: &tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
        owed: u32,
        agent_cancel: &tokio_util::sync::CancellationToken,
        action: IdleAction,
        handed_off: &mut bool,
    ) -> bool {
        if action == IdleAction::Nothing {
            return false;
        }
        let exited = self
            .exit_idle_subprocess_unless_owed(thread_id, msg_rx, owed, agent_cancel, handed_off)
            .await;
        exited && action == IdleAction::EndSession
    }

    /// At an idle, exit the subprocess unless a follow-up is on its way: one in
    /// `msg_rx`, one of the `owed` inputs the agent has not read, or an armed
    /// redirect. The next turn then resumes with `--resume`. Returns whether
    /// it exited.
    ///
    /// Refreshes the run loop's `handed_off` flag. A handoff that ended, for
    /// example by a Stop, re-opens the input arm for what it left queued.
    async fn exit_idle_subprocess_unless_owed(
        &self,
        thread_id: Uuid,
        msg_rx: &tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
        owed: u32,
        agent_cancel: &tokio_util::sync::CancellationToken,
        handed_off: &mut bool,
    ) -> bool {
        // Read before the lock: a query must not hold it.
        *handed_off = crate::engine::successor::handoff_pending(self.pool(), thread_id).await;
        let handed_off = *handed_off;
        // Hold the `agent_sessions` lock across the whole read-decide-act, so
        // the chat fast path's check-and-send takes the same lock and cannot
        // interleave. Otherwise a follow-up could `msg_tx.send` into a
        // subprocess this is about to cancel, and the message is silently
        // dropped. The lock also makes `msg_rx.len()` exact rather than a
        // sample. Only the lock acquire awaits, so this cannot deadlock.
        let mut sessions = self.agent_sessions.lock().await;
        // Taken, not read: one idle of grace, so an arming caller that dies
        // before routing costs one kept-alive idle rather than a pinned
        // subprocess.
        let redirect_pending = sessions
            .get_mut(&thread_id)
            .map(|s| std::mem::take(&mut s.redirect_followup_pending))
            .unwrap_or(false);
        match terminate_decision(msg_rx.len(), owed, redirect_pending, handed_off) {
            TerminateDecision::KeepAliveForFollowup {
                queued,
                unread,
                redirect_pending,
            } => {
                log!("[AgentSession] Skipping subprocess termination for thread {}: a follow-up is still on its way ({} queued, {} unread, redirect pending: {})", thread_id, queued, unread, redirect_pending);
                false
            }
            TerminateDecision::Terminate => {
                // Mark the session exited BEFORE cancelling. The process stays
                // alive for the graceful-shutdown window. A follow-up landing
                // there must see `process_exited` and route through the slow
                // `--resume` path rather than `msg_tx` into a dying
                // subprocess. Mirrors the mark in `completion.rs`.
                if let Some(s) = sessions.get_mut(&thread_id) {
                    s.process_exited = true;
                    s.idle_notify.notify_waiters();
                }
                log!("[AgentSession] Idle reached — terminating the coding-agent subprocess for thread {} so next turn resumes via --resume", thread_id);
                agent_cancel.cancel();
                true
            }
        }
    }

    /// After a hard stop, record the usage Claude Code's driver flushes for a
    /// call cut off mid-stream. The provider billed its prompt, and the event
    /// loop no longer reads. Reads up to `OutputEnded`, dropping everything
    /// else, which the stopped loop would have dropped too.
    #[allow(clippy::too_many_arguments)]
    async fn record_usage_flushed_at_stop(
        &self,
        events_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        thread_id: Uuid,
        token: &std::sync::atomic::AtomicBool,
        meta: &crate::engine::thread_events::EventMeta,
        coding_agent: CodingAgent,
        session_model: Option<&str>,
        spawn_effort: Option<&str>,
    ) {
        if coding_agent != CodingAgent::ClaudeCode {
            return;
        }
        for (model, reported) in usage_flushed_at_stop(events_rx, USAGE_FLUSH_WAIT).await {
            self.record_coding_agent_usage(
                thread_id,
                token,
                meta,
                coding_agent,
                CallModels::of(session_model, model.as_deref()),
                reported,
                spawn_effort,
            )
            .await;
        }
    }

    /// Flush any reasoning accumulated in `buf` past `last_len` as a
    /// `CodingAgentThoughtStreamed`, advancing `last_len`. Idempotent: it emits
    /// only the new tail. Coalesces per-token reasoning deltas into a handful of
    /// rows per turn, mirroring the assistant-text flush path. The buffer is
    /// sliced on a char boundary, so multi-byte reasoning never panics.
    async fn flush_coding_agent_thought(
        &self,
        thread_id: Uuid,
        buf: &str,
        last_len: &mut usize,
        coding_agent: crate::runtime::CodingAgent,
        meta: &crate::engine::thread_events::EventMeta,
    ) {
        if buf.len() <= *last_len {
            return;
        }
        let delta = &buf[buf.floor_char_boundary(*last_len)..];
        if delta.is_empty() {
            return;
        }
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentThoughtStreamed {
                        text: delta.to_string(),
                        coding_agent,
                    },
                    meta: meta.clone(),
                },
                "[AgentSession] CodingAgentThoughtStreamed",
            )
            .await;
        *last_len = buf.len();
    }

    // Bridges every per-session input to the agent runtime; a builder would
    // just shuffle the same fields.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_direct_agent(
        &self,
        // Borrowed, so a caller's stale-resume retry runs on the same claim
        // and nothing can take the thread between the two runs.
        claim: &mut crate::engine::agent_session::spawn_claim::SpawnClaim,
        request_id: Uuid,
        user_message: &str,
        user_images: Option<&[crate::api::ChatImage]>,
        origin_id: Uuid,
        // More events the first prompt carries beside its origin, such as the
        // messages a resume delivers. Its read settles each one.
        carried_event_ids: &[Uuid],
        spawning_event_id: Option<Uuid>,
        cancel_token: &tokio_util::sync::CancellationToken,
        // A conflict-resolution session holds the change's apply claim for
        // its whole run, so no second apply starts beside it.
        conflict: Option<&crate::engine::change_ops::apply_claim::ApplyClaim>,
        recovery_worktree: Option<(PathBuf, String)>,
        repo_id: Option<String>,
        system_prompt_override: Option<String>,
        resume_session_id: Option<String>,
        cc_model: Option<String>,
        cc_reasoning_effort: Option<String>,
        // CWD for `--resume`. See the `UserQuestionAsked.worktree_path` doc.
        resume_worktree_path: Option<PathBuf>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        require_agent_input(user_message, user_images)?;
        let thread_id = claim.thread_id;
        // A run on an open lane batch's lane thread resolves that batch,
        // whoever started it: the lane, a follow-up, a Continue or a switch
        // resume (ADR 0463). A turn the lane sent for a batch that has
        // since ended is refused.
        let lane_duty = if conflict.is_none() && recovery_worktree.is_none() {
            crate::engine::landing_lane::lane_duty_for_run(
                self.pool(),
                self.workspace_path(),
                self.repo_root(),
                thread_id,
                origin_id,
            )
            .await?
        } else {
            None
        };
        let lane_batch = lane_duty.as_ref().map(|duty| duty.batch_id);
        let mut lane_answered = false;
        let result = self
            .run_agent_session(
                claim,
                request_id,
                user_message,
                user_images,
                origin_id,
                carried_event_ids,
                spawning_event_id,
                cancel_token,
                conflict,
                lane_duty,
                &mut lane_answered,
                recovery_worktree,
                repo_id,
                system_prompt_override,
                resume_session_id,
                cc_model,
                cc_reasoning_effort,
                resume_worktree_path,
            )
            .await;
        // A run carrying a batch owes the lane one answer, however it ended.
        // Its completion gives it. Every exit before that answers here.
        if let Some(batch_id) = lane_batch.filter(|_| !lane_answered) {
            let error = result.as_ref().err().map(|e| e.to_string());
            let shutting_down = self.thread_session_is_shutting_down(thread_id).await;
            let outcome = error.as_deref().map_or(Ok(()), Err);
            if let Some(end) =
                crate::engine::landing_lane::unanswered_run_end(outcome, shutting_down)
            {
                self.report_lane_thread_end(batch_id, thread_id, end);
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_session(
        &self,
        claim: &mut crate::engine::agent_session::spawn_claim::SpawnClaim,
        request_id: Uuid,
        user_message: &str,
        user_images: Option<&[crate::api::ChatImage]>,
        origin_id: Uuid,
        carried_event_ids: &[Uuid],
        spawning_event_id: Option<Uuid>,
        cancel_token: &tokio_util::sync::CancellationToken,
        conflict: Option<&crate::engine::change_ops::apply_claim::ApplyClaim>,
        lane_duty: Option<crate::engine::landing_lane::LaneDuty>,
        lane_answered: &mut bool,
        recovery_worktree: Option<(PathBuf, String)>,
        repo_id: Option<String>,
        system_prompt_override: Option<String>,
        resume_session_id: Option<String>,
        cc_model: Option<String>,
        cc_reasoning_effort: Option<String>,
        resume_worktree_path: Option<PathBuf>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let cc_start = std::time::Instant::now();
        let thread_id = claim.thread_id;
        // A retry's first run cancelled the token to kill its process.
        if claim.entry.agent_cancel.is_cancelled() {
            claim.entry.agent_cancel = tokio_util::sync::CancellationToken::new();
        }
        // Resolved before the spawn so the process starts with them, and the
        // shadowed `cc_model` tells the Init handler a model was picked. A
        // pick between turns is in the thread's settings (ADR 0542).
        let (model, effort) = self
            .coding_agent_rung(
                thread_id,
                claim.entry.coding_agent,
                cc_model,
                cc_reasoning_effort,
            )
            .await;
        // A keep-alive's spawn takes no lock against a pick, so its settings
        // emits carry no rung: a pick made meanwhile stays the newest.
        let records_rung = !self.event_bus.in_keep_alive_turn(thread_id);
        let (cc_model, pinned_effort) = {
            let mut sessions = self.agent_sessions.lock().await;
            let session = sessions
                .get_mut(&thread_id)
                .filter(|s| s.msg_tx.same_channel(&claim.entry.msg_tx));
            // Published at once: the commands API reads the entry while the
            // worktree is still being set up.
            if let Some(session) = session {
                session.agent_cancel = claim.entry.agent_cancel.clone();
                session.current_model = model.clone();
                session.current_reasoning_effort = effort.clone();
            }
            (model, effort)
        };
        let conflict_change_id = conflict.map(|c| c.change_id());
        // Neither kind proposes its branch: a resolution lands through Apply
        // or the lane, and ends its session at the first idle that owes no
        // follow-up.
        let is_resolution_session = conflict_change_id.is_some() || lane_duty.is_some();
        let thread_id_str = thread_id.to_string();
        let coding_agent = claim.entry.coding_agent;
        // Pre-spawn app-coding-agent-thread detection. `spawn_agent_thread`
        // stashes the app id here when the LLM picks an app folder. Pop it once,
        // so the worktree dispatcher routes to sparse-checkout and the
        // system-prompt selector picks the app variant. Falls back to
        // `thread_summaries` on resume, so a follow-up on an existing app thread
        // still routes correctly.
        let app_spawn_id: Option<String> = {
            let mut guard = self
                .pending_app_spawn
                .lock()
                .expect("pending_app_spawn poisoned");
            guard.remove(&thread_id)
        };
        let app_spawn_id = if app_spawn_id.is_some() {
            app_spawn_id
        } else {
            // Resume path: read from `thread_summaries` and require kind 'app'.
            // A NULL folder is refused below rather than reconstructed. A failed
            // read refuses too: reading it as "not an app" would root an app
            // thread's session in the Lucidos source repo.
            match sqlx::query_as::<_, (Option<String>, Option<String>)>(
                "SELECT coding_agent_kind, coding_agent_folder FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_optional(&self.pool)
            .await?
            {
                Some((Some(k), folder)) if k == "app" => {
                    if let Some(f) = folder {
                        std::path::Path::new(&f)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .map(str::to_string)
                    } else {
                        // Folder is NULL but kind is 'app'. Reconstructing the
                        // id from disk would race the projection, so refuse and
                        // let the caller retry once the row settles.
                        log!(
                            "[AgentSession] thread {} has coding_agent_kind='app' but NULL folder — \
                             folder reconstruction would race the projection; refusing to spawn",
                            thread_id
                        );
                        return Err(
                            "Thread is marked as an app coding-agent thread but its \
                             coding_agent_folder is missing — wait for the projection \
                             to settle or re-archive the thread."
                                .into(),
                        );
                    }
                }
                _ => None,
            }
        };
        let is_app_spawn = app_spawn_id.is_some();
        // The child's first message recorded the flag, before any run.
        let integrated =
            crate::engine::integrated::thread_is_integrated(&self.pool, thread_id).await;

        // Mutable so the interrupt arm can stamp `meta.actor` with the device
        // that clicked Cancel. The terminal `ResponseCanceled` then carries it
        // for the Initiator popover. Cleared at the next turn boundary, so a
        // resumed session does not inherit a stale actor.
        let mut meta = crate::engine::thread_events::EventMeta {
            request_event_id: Some(origin_id),
            channel: Some(EventChannel::ClaudeCode),
            ..crate::engine::thread_events::EventMeta::NONE
        };

        // The thread's pinned account: the config dir of its FIRST session.
        // Computed once here and injected on EVERY spawn below, so a live
        // `CLAUDE_CONFIG_DIR` toggle can never move an existing thread to
        // another provider. It also scopes the auto-detected resume session id
        // to this account.
        let recorded_pin =
            crate::engine::agent_session::lookup_pinned_cc_config_dir(self.pool(), thread_id)
                .await?;

        // Keeps the computer awake until the session's process is gone. A
        // session parked on a question still holds: its answer often comes
        // from a phone, which an asleep Mac cannot receive.
        let _awake = crate::core::keep_awake::hold(
            crate::core::keep_awake::Work::AgentSession,
            thread_id.to_string(),
        );

        // A conflict-resolution or orphan-recovery session never looks up a
        // session to resume, so it records no continuity either.
        let looks_up_resume = recovery_worktree.is_none() && !is_resolution_session;
        // Its first session is the thread's *successor session*.
        let pending_handoff = if looks_up_resume {
            crate::engine::successor::pending_handoff(self.pool(), thread_id).await?
        } else {
            None
        };
        let (resume_session_id, resume_branch) = if looks_up_resume {
            resolve_resume_context(
                self.pool(),
                self.changes(),
                thread_id,
                resume_session_id,
                recorded_pin.as_ref().map(|pin| pin.dir.as_str()),
                pending_handoff.is_some(),
            )
            .await
        } else {
            (resume_session_id, None)
        };

        // Conflict resolution mode: run in the merge worktree (not repo root)
        let conflict_change = if let Some(cid) = conflict_change_id {
            Some(
                self.changes()
                    .get_by_id(cid)
                    .await?
                    .ok_or("Conflict change not found")?,
            )
        } else {
            None
        };

        log!(
            "[AgentSession] [TIMING] resume lookup: {:?}",
            cc_start.elapsed()
        );

        // If no repo_id was provided (e.g. follow-up message), look up the thread's
        // stored repo from thread_summaries so we stay bound to the original repo.
        let repo_id = if repo_id.is_none() {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT cc_repo_id FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_optional(&self.pool)
            .await?
            .flatten()
        } else {
            repo_id
        };

        // Coding-agent sessions must branch from main, not from a stale worktree branch.
        let dev_root = main_worktree().await;

        // Explicit repo_id wins; otherwise fall back to the workspace's registered
        // Lucidos repo so SessionStarted always names a real repository. Without
        // this, the route panel rendered the workspace name as if it were the repo.
        let repo = if let Some(ref rid) = repo_id {
            let repo_uuid = Uuid::parse_str(rid)?;
            Some(
                crate::core::repositories::RepositoryStore::get(&self.pool, repo_uuid)
                    .await?
                    .ok_or_else(|| format!("Repository {} not found", rid))?,
            )
        } else {
            crate::core::repositories::RepositoryStore::get_by_name(
                &self.pool,
                Self::DEFAULT_REPO_NAME,
            )
            .await?
        };

        let (repo_id, repo_root, is_external_repo, external_repo_name, repo_name) = if is_app_spawn
        {
            // App coding-agent thread: the worktree's git root is the workspace
            // itself, and apps are not in the repo registry.
            (None, self.workspace_path.clone(), false, None, None)
        } else if let Some(repo) = repo {
            let path = PathBuf::from(&repo.path);
            if !path.exists() {
                return Err(format!("Repository path does not exist: {}", repo.path).into());
            }
            let is_external = is_external_repo_path(&path, &dev_root);
            let repo_name = repo.name.clone();
            let external_repo_name = if is_external { Some(repo.name) } else { None };
            (
                Some(repo.id.to_string()),
                path,
                is_external,
                external_repo_name,
                Some(repo_name),
            )
        } else {
            // No repo resolved and not an app spawn, so this is "edit Lucidos
            // itself". Only legitimate when a source checkout exists.
            (
                None,
                unregistered_lucidos_root(dev_root, crate::paths::has_lucidos_source())?,
                false,
                None,
                None,
            )
        };
        let session_kind = if is_app_spawn {
            crate::engine::agent_session::CodingAgentKind::App
        } else if is_external_repo {
            crate::engine::agent_session::CodingAgentKind::External
        } else {
            crate::engine::agent_session::CodingAgentKind::Lucidos
        };

        let workspace_name = self.workspace_name();
        // The tri-state matters only to spawn-time branch adoption, which reads
        // "no idle recorded one" and "could not ask" differently. Everything
        // else below wants the sha or nothing.
        let idle_anchor = crate::engine::agent_session::resume::lookup_latest_worktree_head_sha(
            self.pool(),
            thread_id,
        )
        .await;
        let last_idle_sha = idle_anchor.found().map(str::to_string);
        let SpawnWorktreeContext {
            cwd,
            system_prompt,
            mut session_context,
            mut branch_name,
            worktree_path,
            interactive_session,
            branch_note,
            resume_session_id,
            branch_gone,
            worktree_created,
            branch_created,
        } = self
            .resolve_run_worktree_context(
                recovery_worktree,
                &conflict_change,
                lane_duty.as_ref(),
                system_prompt_override,
                &app_spawn_id,
                is_app_spawn,
                is_external_repo,
                external_repo_name,
                repo_name.clone(),
                &workspace_name,
                &repo_root,
                &repo_id,
                &idle_anchor,
                resume_worktree_path,
                resume_branch,
                resume_session_id,
                thread_id,
                cc_start,
                coding_agent,
            )
            .await?;

        // Anchor for idle-time branch adoption: where this session's worktree
        // sat BEFORE the agent ran. The previous idle's HEAD is the stronger
        // anchor, because it already contains the thread's commits. But it is
        // `None` on a first turn, which is exactly the single-turn session that
        // renames its own branch and can never recover. Reading HEAD here costs
        // one `rev-parse` and gives that case an anchor.
        // `try_adopt_branch_at_idle` carries the gate that makes it safe.
        let adoption_anchor_sha = match worktree_path.as_deref() {
            Some(wt) => crate::engine::agent_session::external_edits::git_head_sha(wt)
                .await
                .or_else(|| last_idle_sha.clone()),
            None => last_idle_sha.clone(),
        };

        // A resolution session works for the batch or the change, never as the
        // integrated thread itself.
        if integrated && !is_resolution_session {
            session_context.push(
                &crate::engine::agent_session::prompts::integrated_section(self.pool(), thread_id)
                    .await,
            );
        }

        // Asked before the process starts, so a failed lookup costs no work.
        // This session's `SessionStarted` closes the window, so it must not be
        // recorded over a lookup that failed. A lookup that fails after its
        // retries fails the turn: starting anyway would lose the message for
        // good, while a failed turn is retried by the next send (ADR 0574).
        // A keep-alive must not act.
        let first_prompt_events: Vec<Uuid> = std::iter::once(origin_id)
            .chain(carried_event_ids.iter().copied())
            .collect();
        let stranded = if self.event_bus.in_keep_alive_turn(thread_id) {
            Vec::new()
        } else {
            crate::engine::agent_session::stranded_follow_ups::stranded_follow_ups(
                &self.pool,
                self.workspace_path(),
                thread_id,
                &first_prompt_events,
            )
            .await
            .map_err(|e| {
                format!("Could not check for messages the previous session never read: {e}")
            })?
        };
        // A new session in an existing thread sees what was discussed and done
        // before this message, and the images no agent has seen yet. A stranded
        // follow-up is redelivered with its own images, so it brings none here.
        // A successor session starts clean: its opening carries the context.
        if pending_handoff.is_none() {
            let thread_messages = self.event_store.get_thread_messages(&thread_id_str).await?;
            let (prior, current_at) = before_current_message(
                thread_messages.iter().map(HistoryLine::of).collect(),
                user_message,
            );
            let redelivered: Vec<String> =
                stranded.iter().map(|f| f.event_id.to_string()).collect();
            let mut attached = Vec::new();
            let mut images = Vec::new();
            for hash in unanswered_image_hashes(&prior[..current_at], &redelivered) {
                match crate::core::blobs::read_blob_as_base64(self.workspace_path(), hash) {
                    Some((base64, mime_type)) => {
                        attached.push(hash);
                        images.push(crate::api::ChatImage { base64, mime_type });
                    }
                    None => log!(
                        "[AgentSession] Image {} of thread {} is gone; the fresh session goes without it",
                        hash,
                        thread_id
                    ),
                }
            }
            session_context.push(&thread_history_block(&prior, &attached));
            session_context.attach_images(images);
        }
        // A successor session reads its opening first, whatever input started it.
        let opened_message = pending_handoff
            .as_ref()
            .map(|handoff| handoff.first_message(user_message));
        let first_message = opened_message.as_deref().unwrap_or(user_message);
        // A keep-alive carries no notes: the agent must not act on anything.
        let first_prompt = if self.event_bus.in_keep_alive_turn(thread_id) {
            PromptFor::KeepAlive(first_message)
        } else {
            PromptFor::Turn(first_message)
        };
        let session_context = session_context.render();

        // The startup semaphore limits concurrent process initializations. Hold
        // the permit until Init, when the process is up and mostly idle.
        let startup_cancel = claim.entry.startup_cancel.clone();
        let startup_permit = tokio::select! {
            biased;
            _ = startup_cancel.cancelled() => None,
            permit = self.cc_startup_semaphore.clone().acquire_owned() => {
                Some(permit.map_err(|e| format!("Startup semaphore closed: {}", e))?)
            }
        };

        if resume_session_id.is_some() {
            log!("[AgentSession] Resuming session for thread {}", thread_id);
        }
        let agent_cancel = claim.entry.agent_cancel.clone();
        let allowed_tools = crate::engine::claude_code::cc_allowed_tools(&self.grants_dir());
        // User-managed env vars injected into the coding-agent subprocess.
        // Applied first in `apply_lucidos_env`, so engine-owned vars still win.
        let user_env_vars =
            crate::core::EnvironmentVariableStore::spawn_pairs(&self.pool, "AgentSession").await;
        // Masked out of every tool result this session stores.
        let tool_result_secrets =
            crate::runtime::spawn_env::session_secret_values(thread_id, &user_env_vars);
        // The account pin this session runs under. A CC session's transcript
        // lives at `$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl`, and the
        // thread is pinned to the profile of its FIRST session.
        //   * `injected_pin` is replayed on EVERY spawn of an existing thread,
        //     so a live `CLAUDE_CONFIG_DIR` toggle never moves it to another
        //     account. `None` only on the first turn, which sets the pin.
        //   * `effective_pin` is what CC ACTUALLY runs under this spawn, recorded
        //     at Init: the replayed pin, else what turn 1 inherits.
        let injected_pin = recorded_pin
            .as_ref()
            .map(|recorded| recorded.resolve(&user_env_vars));
        let effective_pin = injected_pin.clone().or_else(|| {
            crate::runtime::AccountPin::for_first_session(
                live_claude_config_dir(&user_env_vars),
                default_claude_config_dir(),
            )
        });
        // CC's defaults, resolved as the spawned CC will resolve them. The
        // settings files are a CC-only fallback, reached only for a model the
        // registry has no default effort for: their effort vocabulary must not
        // leak into other backends.
        let cc_defaults = (coding_agent == CodingAgent::ClaudeCode).then(|| {
            crate::runtime::claude_code::CcSettingsScope {
                env: &user_env_vars,
                inherited: crate::core::inherited_env_var,
                config_dir: effective_pin
                    .as_ref()
                    .map(|pin| std::path::Path::new(pin.dir())),
                project_dir: &cwd,
            }
        });
        let cc_reasoning_effort =
            pinned_effort.or_else(|| cc_defaults.as_ref().and_then(|s| s.default_effort()));
        // Only a hint for the Init handler: CC names the model it chose at
        // Init, but strips a `[1m]` suffix the user's own setting carries.
        let cc_default_model = cc_defaults.as_ref().and_then(|s| s.default_model());
        // User-configured agent binary path. Resolved here, where the spawn
        // orchestration has the pool, and validated inside the runtime's spawn.
        // An unresolvable path fails loud and names the setting, rather than
        // falling back to probing.
        let binary_override_pref = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => {
                &crate::core::prefs::CODING_AGENT_CLAUDE_PATH
            }
            crate::runtime::CodingAgent::Codex => &crate::core::prefs::CODING_AGENT_CODEX_PATH,
        };
        let binary_override = binary_override_pref.read(self.pool()).await;
        // Whether the text the model writes before a tool call reaches the
        // user. Decided once per spawn, so the prompt stays fixed and the
        // stream parser agrees with it. Claude Code only: Codex streams its
        // own summary.
        let notes = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => {
                crate::runtime::claude_code_provider::notes_visibility(
                    &crate::runtime::claude_code_provider::ProviderInputs {
                        binary_override: binary_override.as_deref(),
                        user_env_vars: &user_env_vars,
                        account_pin: injected_pin.as_ref(),
                        worktree_path: &cwd,
                        thread_id,
                    },
                )
                .await
            }
            crate::runtime::CodingAgent::Codex => {
                crate::runtime::claude_code_provider::NotesVisibility::Shown
            }
        };
        let system_prompt =
            crate::engine::agent_session::prompts::with_notes_visibility(system_prompt, notes);
        // Which of Claude Code's permission modes this session runs in. Read
        // only for Claude Code: Codex has no equivalent and ignores the field.
        // An unreadable preference means the default, so a DB hiccup costs the
        // user their opt-in for one spawn and never the spawn itself.
        let permission_mode = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => {
                crate::core::prefs::CODING_AGENT_CLAUDE_PERMISSION_MODE
                    .stored(self.pool())
                    .await
            }
            crate::runtime::CodingAgent::Codex => None,
        };
        // The auto-compact trial reaches Claude Code only. The same value feeds
        // the spawn env and the `SessionStarted` record below. A temporary
        // measure: docs/temporary-measures.md § "Auto-compact trial".
        let auto_compact = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => {
                crate::engine::auto_compact_trial::resolve(self.pool(), thread_id, &user_env_vars)
                    .await
            }
            crate::runtime::CodingAgent::Codex => None,
        };
        let repo_directory_grants = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => {
                self.remember_cc_system_prompt(thread_id, &system_prompt);
                crate::engine::repo_directory_grants::resolve(&cwd, self.workspace_path()).await
            }
            crate::runtime::CodingAgent::Codex => Vec::new(),
        };
        // A Stop during the semaphore wait, or the reads since, costs nothing
        // yet: no process exists.
        let Some(startup_permit) = startup_permit.filter(|_| !startup_cancel.is_cancelled()) else {
            crate::engine::git_ops::cleanup_failed_spawn(
                &repo_root,
                worktree_path.as_deref(),
                &branch_name,
                worktree_created,
                branch_created,
            )
            .await;
            return Ok(self.settle_canceled_startup(claim, request_id, &meta).await);
        };
        log!(
            "[AgentSession] [TIMING] Startup semaphore acquired: {:?}",
            cc_start.elapsed()
        );
        let (runtime, resume_attempt) = match spawn_or_resume(
            self,
            coding_agent,
            crate::runtime::SpawnArgs {
                worktree_path: &cwd,
                coding_agent_kind: session_kind,
                workspace_path: self.workspace_path(),
                allowed_tools: Some(&allowed_tools),
                system_prompt: Some(&system_prompt),
                session_context: Some(&session_context),
                resume_session_id: resume_session_id.as_deref(),
                model: cc_model.as_deref(),
                reasoning_effort: cc_reasoning_effort.as_deref(),
                thread_id,
                spawning_event_id,
                repo_name: repo_name.as_deref(),
                interactive: interactive_session,
                user_env_vars: &user_env_vars,
                binary_override: binary_override.as_deref(),
                permission_mode: permission_mode.as_deref(),
                account_pin: injected_pin.as_ref(),
                additional_directories: &repo_directory_grants,
                notes,
                auto_compact,
                agent_run: crate::runtime::agent_run_marker::AgentRun::for_thread(
                    self.workspace_path(),
                    thread_id,
                    self.teardown_handover.clone(),
                ),
            },
            agent_cancel.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                // Remove ONLY what this attempt created. Running both git calls
                // unconditionally lets a failed RESUME force-remove the
                // pre-existing worktree and delete the branch holding the
                // thread's committed work.
                crate::engine::git_ops::cleanup_failed_spawn(
                    &repo_root,
                    worktree_path.as_deref(),
                    &branch_name,
                    worktree_created,
                    branch_created,
                )
                .await;
                return Err(format!("Failed to start the coding agent: {}", e).into());
            }
        };

        log!(
            "[AgentSession] [TIMING] CC process spawned: {:?}",
            cc_start.elapsed()
        );

        let crate::runtime::RunningAgent {
            mut events_rx,
            input_tx: agent_input_tx,
            control_tx: agent_control_tx,
            withdraw_tx: agent_withdraw_tx,
            kind: _,
            // In-band approval requests (Codex app-server). `None` for CC and
            // the Codex exec escape hatch, where the matching select arm below
            // pends forever. Mutable so a closed channel can disable the arm.
            permission_rx: mut agent_permission_rx,
        } = runtime;

        // AskUserQuestion answers are sent as plain user messages, never
        // `tool_result` blocks. Resuming an unfinished tool_use auto-injects a
        // synthetic pair BEFORE processing stdin. That would orphan any
        // `tool_result` we sent, and the LLM would re-ask the same question.
        let images = user_images.map(|imgs| imgs.to_vec()).unwrap_or_default();

        // Phase 8.2: detect external user edits made between turns and prepend
        // a short note so CC reacts instead of being surprised. Only fires when:
        //   - this thread has at least one prior `CodingAgentIdled` event with a
        //     recorded `worktree_head_sha` (skips truly-first spawns, where
        //     there's no SHA to compare against)
        //   - the worktree has actually changed since the recorded SHA (no
        //     diff → no note, see helper)
        //
        // The note is prepended to the text only. Images are forwarded as-is.
        // Failures inside the helper degrade silently to "no note", matching
        // the rest of the resume code's tolerance for best-effort git
        // introspection.
        let first = first_prompt_for_agent(
            &self.pool,
            &self.event_store,
            thread_id,
            origin_id,
            first_prompt,
            ResumeSpawnContext {
                worktree_path: worktree_path.as_deref(),
                last_idle_sha: last_idle_sha.as_deref(),
                branch_note: branch_note.as_deref(),
                session_branch: Some(branch_name.as_str()),
            },
            HeldBlocks::for_agent(
                &self.sent_device_contexts,
                coding_agent,
                resume_session_id.as_deref(),
                &[],
            ),
        )
        .await;
        let first_input = AgentInput {
            text: first.text,
            images,
            uuid: Uuid::new_v4(),
        };
        // Inputs carrying the device block, until the agent reads them.
        let mut blocks_in_flight: HashMap<Uuid, ContextFingerprint> = HashMap::new();
        if let Some(fingerprint) = first.device_block {
            blocks_in_flight.insert(first_input.uuid, fingerprint);
        }
        // The first prompt is the first owed input.
        let mut inputs = InputLedger::new();
        inputs.forwarded(first_prompt_events, &first_input, None);
        if agent_input_tx.send(first_input).is_err() {
            // Same contract as the spawn_or_resume failure arm above: remove
            // ONLY what this attempt created: a resumed worktree/branch holds
            // the thread's committed work.
            crate::engine::git_ops::cleanup_failed_spawn(
                &repo_root,
                worktree_path.as_deref(),
                &branch_name,
                worktree_created,
                branch_created,
            )
            .await;
            // The driver already wound down, so recover the real cause it
            // flushed onto `events_rx`. A bare "channel closed" hides what
            // actually failed.
            let death = drain_startup_death(&mut events_rx);
            return Err(
                match classify_startup_failure(resume_session_id.is_some(), death.reason.clone()) {
                    StartupFailure::Failed(plain) => {
                        startup_failure_error(coding_agent, death, plain)
                    }
                    StartupFailure::StaleResume => {
                        log!(
                            "[AgentSession] thread {} could not resume sid={}: the coding agent has no such session. Retrying with a fresh one.",
                            thread_id,
                            resume_session_id.as_deref().unwrap_or("")
                        );
                        // Same contract as the run loop's stale-resume arm:
                        // shadow the dead id so the retry cannot pick it
                        // again. The driver already exited, so there is no
                        // process group to wait for.
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event:
                                        crate::engine::thread_events::ThreadEvent::SessionEnded {
                                            reason: SessionEndReason::StaleResume,
                                        },
                                    meta: meta.clone(),
                                },
                                "[AgentSession] SessionEnded (stale resume at startup)",
                            )
                            .await;
                        STALE_RESUME_ERROR.into()
                    }
                },
            );
        }

        let mut startup_permit = Some(startup_permit);

        let mut pending_withdraws = PendingWithdraws::new();
        // Withdraws for follow-ups still in `msg_rx`, answered once forwarded.
        let mut withdraws_awaiting_forward: Vec<WithdrawInputRequest> = Vec::new();
        let stop = claim.entry.stop.clone();
        let interrupt = claim.entry.interrupt.clone();
        let idle_notify = claim.entry.idle_notify.clone();
        let shutting_down = claim.entry.shutting_down.clone();
        let external_terminal_emitted = claim.entry.external_terminal_emitted.clone();
        let external_continuation_requested = claim.entry.external_continuation_requested.clone();
        let mut normalized_model = cc_model.clone();
        // Shared with the session entry, so the external watchdog reads the
        // same atomic the loop below mutates.
        let tools_in_flight_shared = claim.entry.tools_in_flight.clone();
        let question_flushes = LoopEndsFlushes(claim.entry.question_flushes.clone());

        // The process runs: the claimed entry turns `Running`, in the same lock
        // hold that re-checks for a Stop that landed during the spawn.
        let canceled_during_spawn = {
            let mut sessions = self.agent_sessions.lock().await;
            let Some(session) = sessions
                .get_mut(&thread_id)
                .filter(|s| s.msg_tx.same_channel(&claim.entry.msg_tx))
            else {
                agent_cancel.cancel();
                return Err("The coding agent lost its claim on the thread while starting.".into());
            };
            if startup_cancel.is_cancelled() {
                true
            } else {
                session.phase = crate::engine::SessionPhase::Running {
                    control_tx: agent_control_tx.clone(),
                };
                // Names this session as the resolver for the merge-ownership
                // guard (ADR 0060). Tier-2 / Tier-3 merge spawns reach here
                // with `conflict_change_id` set, and a lane thread with its
                // batch. Every other session is None.
                session.conflict = conflict_change_id
                    .map(|change_id| crate::engine::types::ConflictBinding::Detached { change_id })
                    .or_else(|| {
                        lane_duty.as_ref().map(|duty| {
                            crate::engine::types::ConflictBinding::LaneBatch {
                                batch_id: duty.batch_id,
                            }
                        })
                    });
                session.worktree_path = worktree_path.clone();
                session.branch_name = Some(branch_name.clone());
                session.repo_root = Some(repo_root.clone());
                session.current_model = normalized_model.clone();
                session.current_reasoning_effort = cc_reasoning_effort.clone();
                session.reasoning_effort_in_force = cc_reasoning_effort.clone();
                session
                    .last_event_at
                    .store(now_epoch_millis(), std::sync::atomic::Ordering::Relaxed);
                false
            }
        };
        if canceled_during_spawn {
            // The claim is held until the process is gone, so no next spawn
            // resumes into the worktree beside the dying one.
            agent_cancel.cancel();
            await_process_teardown(&mut events_rx, thread_id, "a Stop during startup").await;
            return Ok(self.settle_canceled_startup(claim, request_id, &meta).await);
        }
        // A dropped run future now leaves a turn that nobody will end.
        claim.entry_guard.settle_if_dropped();
        if let Some(apply_claim) = conflict {
            apply_claim.carried_by_session(external_continuation_requested.clone());
        }
        let msg_tx = claim.entry.msg_tx.clone();
        let crate::engine::agent_session::spawn_claim::ClaimedEntry {
            msg_rx,
            withdraw_rx,
            child_reports,
            ..
        } = &mut claim.entry;

        self.redeliver_stranded_follow_ups(thread_id, &msg_tx, msg_rx, stranded)
            .await;

        let chat_cancel = cancel_token.clone();
        let images: Vec<String> = Vec::new();

        // Emit SessionStarted immediately so the branch→thread mapping exists
        // before CC produces any output. Without this, an engine crash during CC
        // initialization leaves no mapping and recovery creates orphan threads.
        // The cc_session_id is not yet known (comes from CC's Init event), but
        // recovery uses CodingAgentIdled for --resume, not SessionStarted.
        let session_folder = if let Some(ref app_id) = app_spawn_id {
            self.workspace_path
                .join("data")
                .join("apps")
                .join(app_id)
                .to_string_lossy()
                .to_string()
        } else {
            // Lucidos / External: folder = repo_root. Workspace-local repos
            // (lucidos source under a non-default install root) reuse the
            // same path their SessionStarted event has always carried.
            repo_root.to_string_lossy().to_string()
        };
        // Read before the emit below, which would count as an earlier session.
        let continuity = if looks_up_resume || resume_attempt == ResumeAttempt::Resumed {
            resolve_session_continuity(
                self.pool(),
                thread_id,
                resume_attempt,
                branch_gone,
                pending_handoff.as_ref(),
            )
            .await
        } else {
            None
        };
        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::SessionStarted {
                    session_id: String::new(),
                    branch: branch_name.clone(),
                    repo_id: repo_id.clone(),
                    coding_agent_kind: session_kind,
                    coding_agent_folder: session_folder,
                    app_id: app_spawn_id.clone(),
                    coding_agent,
                    integrated,
                    auto_compact_window: auto_compact.and_then(|s| s.window),
                    auto_compact_arm: auto_compact.map(|s| s.arm),
                    continuity,
                },
                meta: meta.clone(),
            })
            .await
        {
            log!(
                "[AgentSession] Failed to emit initial SessionStarted for {}: {}",
                thread_id,
                e
            );
        }

        // Seed the branch work from the actual worktree state. The
        // projection's per-event handlers keep it live, but cannot fill
        // the gap between session start and the first new commit. A thread
        // resumed after an engine restart would show no Diff button until its
        // next commit, even with commits already on the branch.
        //
        // Outside the projection transaction by design: `git diff` inside a
        // Postgres transaction is the wrong shape. The helper logs and continues
        // on failure, because bootstrap must not block on git. It writes the
        // same git truth the Diff button computes, so the button's visibility
        // and its rendered diff stay in lockstep. Both the initial start and the
        // resume path land here.
        crate::engine::session_seed::seed_branch_work(
            self.pool(),
            thread_id,
            &repo_root,
            &branch_name,
        )
        .await;

        // Persist initial model/effort so cc_thread_settings() can restore them
        // after the session exits. Without this, the frontend loses the model
        // selection when viewing idle threads (no live session to query).
        if records_rung && (normalized_model.is_some() || cc_reasoning_effort.is_some()) {
            if let Err(e) = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentSettingsChanged {
                        model: normalized_model.clone(),
                        reasoning_effort: cc_reasoning_effort.clone(),
                        coding_agent,
                        // Fires before CC's Init, so the session id is not known
                        // yet. The Init handler emits a second SettingsChanged
                        // carrying it and the config dir.
                        cc_session_id: None,
                        claude_config_dir: None,
                        claude_config_dir_explicit: None,
                    },
                    meta: meta.clone(),
                })
                .await
            {
                log!(
                    "[AgentSession] Failed to persist initial CodingAgentSettingsChanged for {}: {}",
                    thread_id,
                    e
                );
            }
        }

        let mut result_texts: Vec<String> = Vec::new();
        // Count tool calls seen before the first `Result`. Feeds
        // `is_stale_resume_signal`: a resumed turn that made ANY tool call is
        // alive and working, even with no assistant text from a terse model. The
        // stale check only fires on the first Result, so a session-scoped
        // counter is exactly "tool calls before the first Result".
        let mut tool_calls_seen: u32 = 0;
        // Count backend-to-model API calls seen before the first `Result`, read
        // as `no_api_call_this_turn` by `is_resume_settle_result`. A `Usage`
        // event is emitted per real API call and ONLY for one, which takes two
        // guards in the parser. It drops all-zero usage frames, which is what a
        // synthetic message carries. And it reports a CC assistant message once
        // on its `message.id`, because CC repeats the same usage per content
        // block.
        //
        // So zero here is positive proof no model call happened. That separates
        // "this Result closes the backend's own resume-settle turn" from "the
        // model was asked our prompt and answered with nothing". The two shapes
        // are otherwise identical, and skipping the second would strand the turn
        // until the inactivity watchdog fired.
        let mut api_calls_seen: u32 = 0;
        // The session id the backend reported at `Init`, meaning the
        // conversation it ACTUALLY attached to. Compared against
        // `resume_session_id` to prove a `--resume` landed on the live
        // conversation, which vetoes the empty-echo stale-resume heuristic
        // outright. Without it, a healthy resume that says nothing is
        // indistinguishable from a dead one.
        let mut init_session_id: Option<String> = None;
        let mut claude_text_buf = CodingAgentTextBuffer::default();
        // Reasoning stream buffer, coalesced like the text buffer above. Deltas
        // arrive per-token, so flush on a paragraph boundary or once
        // `THOUGHT_FLUSH_THRESHOLD` chars accumulate. The threshold bounds
        // latency, so a long unbroken paragraph still streams live.
        let mut claude_thought_buf = String::new();
        let mut last_thought_persisted_len: usize = 0;
        const THOUGHT_FLUSH_THRESHOLD: usize = 240;
        let mut is_waiting = false;
        // The session handed off: it forwards no more input.
        let mut handed_off = false;
        let mut proposed_change = false;
        // The branch HEAD each turn-end re-entry last went out for.
        let mut reentry_ledger = crate::engine::turn_end::ReentryLedger::default();
        let mut emitted_terminal_event = false; // Track whether ResponseGenerated/ResponseCanceled was emitted
                                                // user_hit_stop: when true, the next Result emits ResponseCanceled (exchange:
                                                // "Canceled") instead of ResponseGenerated. Reset on next user follow-up.
        let mut user_hit_stop = false;
        // redirect_cause: set alongside user_hit_stop when a redirect (a
        // follow-up) sent the interrupt. The Result then
        // renders neutrally, not as a user Stop. Cleared at the turn boundary
        // in lockstep with user_hit_stop.
        let mut redirect_cause = None;
        // last_emitted_idle: true iff the most recent in-loop event was
        // CodingAgentIdled. The post-loop relies on this flag to decide whether to
        // synthesize an idle event before SessionEnded.
        let mut last_emitted_idle = false;
        // Paired ToolCalled / ToolResult counter. The watchdog disarms while it
        // is above zero, because tool execution is legitimate silence, not a
        // hang. CC may batch several calls per turn, so a counter rather than a
        // bool is the right shape. Mirrored on `AgentSession` so the external
        // watchdog sees the same atomic from outside this `select!`.
        let tools_in_flight = tools_in_flight_shared;
        let mut tool_names = super::super::tool_output::ToolNamesById::default();
        // last_terminal_kind: terminal emitted by the most recently completed
        // turn. Drives `should_auto_commit_on_cleanup`, so the cleanup commits
        // only when the last turn ended Generated. Every other terminal leaves
        // this None or non-Generated, which means no auto-commit and no spurious
        // Apply card. Reset per turn alongside `emitted_terminal_event`.
        let mut last_terminal_kind: Option<TerminalKind> = None;
        // withheld_api_error: what the released *auto-resume hold* handed back,
        // so the error of a terminal whose completion card the fan-in withheld.
        // The hold itself only spans the terminal and idle emits. This carries
        // both the decision and the announcement on to the exit that actuates
        // it. Written in lockstep with `last_terminal_kind`, and reset with it
        // at a turn boundary, because it describes THAT terminal.
        let mut withheld_api_error: Option<String> = None;
        // Set by the watchdog tick below; consumed by the safety net to
        // pick ContinuationRequested auto-resume vs ResponseAborted. Not derived
        // from `agent_cancel.is_cancelled()` because the stale-resume and
        // question-cancel paths also cancel the token for non-hang reasons.
        let mut watchdog_fired = false;
        // How the agent's process ended, from its `Exited`. The safety net
        // classifies it: a stray signal kill auto-resumes, a failed exit fails
        // the turn with the agent's own words.
        let mut exit_facts = AgentExitFacts::default();
        // Bounded Esc fallback. A real Cancel forwards CC's native interrupt and
        // waits for CC to wind down and emit a `Result`. If CC does not honor it
        // within this window, escalate to the hard stop so Cancel is always
        // responsive. The watchdog skips while a tool is in flight, so it cannot
        // catch a control request ignored during a long tool. Armed when the
        // interrupt is forwarded, cleared once the terminal `Result` lands.
        let mut interrupt_escalate_at: Option<tokio::time::Instant> = None;
        const INTERRUPT_ESCALATE_AFTER: std::time::Duration = std::time::Duration::from_secs(8);

        'event_loop: loop {
            // Snapshot the (Copy) deadline so the escalation arm below polls a
            // value, not a borrow of `interrupt_escalate_at` that other arms mutate.
            let escalate_deadline = interrupt_escalate_at;
            tokio::select! {
                event_opt = events_rx.recv() => {
                    let Some(ev) = event_opt else {
                        // Driver task exited without sending Exited (defensive).
                        log!(
                            "[AgentSession] events_rx closed without AgentEvent::Exited for thread {}",
                            thread_id
                        );
                        break;
                    };
                    // Not agent activity: it must not start a turn. Only a
                    // stopped session acts on it, through
                    // `record_usage_flushed_at_stop`.
                    if matches!(ev, AgentEvent::OutputEnded) {
                        continue;
                    }
                    settle_answered_withdraws(&self.event_bus, thread_id, &mut inputs, &mut pending_withdraws).await;
                    // Only a read that settles an owed input can start a turn. A
                    // replay naming no owed input settles nothing. The read is
                    // announced once the new-turn check below knows whether it
                    // opened a turn.
                    let (read_owed_input, reads) = match inputs.observe(&ev) {
                        Settled::Read(reads) => (true, reads),
                        Settled::Rejected => {
                            log!("[AgentSession] The agent rejected a forwarded input on thread {}; it stays unread", thread_id);
                            (false, Vec::new())
                        }
                        Settled::Nothing => (false, Vec::new()),
                    };
                    child_reports.publish(&inputs);
                    if let AgentEvent::Exited { exit } = ev {
                        exit_facts = exit;
                        // Final flush of any pending reasoning: surface what the
                        // process reasoned before it died.
                        self.flush_coding_agent_thought(
                            thread_id,
                            &claude_thought_buf,
                            &mut last_thought_persisted_len,
                            coding_agent,
                            &meta,
                        )
                        .await;
                        claude_text_buf.flush(&self.event_bus, thread_id, coding_agent, &meta, "[AgentSession] CodingAgentTextStreamed (final flush on exit)").await;
                        if is_waiting {
                            // The process exited after producing a Result, so the
                            // session is idle. Do not hold the ThreadGuard for
                            // follow-ups: auto-commit, drop the sessions entry
                            // and return. The worktree and branch persist on
                            // disk, so a follow-up reuses them, and engine
                            // shutdown has no idle loop to cancel.
                            // Partial work skips the auto-commit, so it never
                            // becomes a commit a later proposal would carry. The
                            // gate stays purely about terminal kind. A Discard recorded by now only
                            // commits what finalize then deletes with the branch.
                            // The commit runs BEFORE the reap, while the entry is
                            // still in the map. A Stop landing meanwhile is then
                            // recorded on it, instead of racing this commit from
                            // the no-session fallback.
                            if let Some(ref wt) = worktree_path {
                                if should_auto_commit_on_cleanup(false, &last_terminal_kind) {
                                    auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Coding agent changes (auto-committed on idle exit)").await;
                                }
                            }

                            let exit = {
                                let mut guard = self.agent_sessions.lock().await;
                                idle_notify.notify_waiters();
                                idle_exit(&mut guard, thread_id, &external_terminal_emitted)
                            };
                            let IdleExit::Release(entry) = exit else {
                                log!("[AgentSession] CC process exited while idle with a stop pending for thread {}; finalize carries it out", thread_id);
                                break;
                            };
                            log!("[AgentSession] CC process exited while idle, releasing thread {}", thread_id);
                            // Save slash commands to cache now the session is gone.
                            // Model/effort are persisted via CodingAgentSettingsChanged events.
                            let cache_snapshot = entry.and_then(|s| {
                                s.repo_root.as_ref().map(|r| {
                                    (r.to_string_lossy().to_string(), s.to_commands_info())
                                })
                            });
                            if let Some((repo_key, info)) = cache_snapshot {
                                self.upsert_cc_commands_cache(repo_key, info).await;
                            }

                            // Drain follow-ups that arrived between CodingAgentIdled
                            // and process exit. Convert to orphaned injections so the
                            // caller re-processes them instead of showing "interrupted".
                            let orphans = lost_followups_to_orphans(drain_lost_followups(msg_rx));

                            // A turn the backend ended on a transient upstream
                            // `API Error` resumes itself, rather than leaving a
                            // red dot nobody is watching. This arm is the site
                            // that matters: a reported drop arrives as a real
                            // Result, so the turn idles and returns HERE and
                            // never reaches the post-loop finalize.
                            //
                            // Position is load-bearing. It runs after this turn's
                            // idle, after the session was dropped just above, and
                            // after the follow-up drain, which decides whether
                            // anything else is coming.
                            self.maybe_auto_resume_after_api_error(
                                thread_id,
                                &meta,
                                withheld_api_error.take(),
                                !orphans.is_empty(),
                                // `idle_exit` released the entry: no stop was pending.
                                false,
                            )
                            .await;

                            return Ok(ProcessResult {
                                response: String::new(),
                                steps: vec![],
                                images,
                                request_id,
                                thread_id,
                                proposed_change,
                                auto_apply: false,
                                orphaned_injections: orphans,
                            });
                        }
                        log!(
                            "[AgentSession] CC exited without Result event for thread {} (buffered_text_len={})",
                            thread_id,
                            claude_text_buf.as_str().len()
                        );
                        break;
                    }
                    // Stamp liveness for apply_now's timeout. Also drain the
                    // question-answer resume signal in the SAME lock. Two turns
                    // start without the run loop forwarding anything at that
                    // moment, so neither reaches the `msg_rx` arm's
                    // `reset_per_turn_flags`:
                    //
                    // - A live subprocess woken by an answered question resumes
                    //   through the PreToolUse hook.
                    // - An input forwarded during the previous turn, read only
                    //   after that turn's `Result` (ADR 0268).
                    //
                    // On a terminal-armed turn, re-arm emission below, so the new
                    // turn's events are processed rather than dropped as
                    // stragglers.
                    let new_turn_after_terminal = {
                        let mut guard = self.agent_sessions.lock().await;
                        if let Some(s) = guard.get_mut(&thread_id) {
                            s.last_event_at.store(now_epoch_millis(), std::sync::atomic::Ordering::Relaxed);
                            let armed = std::mem::take(&mut s.question_resume_pending);
                            let reset = starts_turn_after_terminal(
                                armed,
                                read_owed_input && !s.process_exited,
                                emitted_terminal_event,
                            );
                            if reset {
                                // Mirror the msg_rx arm's session-side clear.
                                s.is_waiting = false;
                            }
                            reset
                        } else {
                            false
                        }
                    };
                    if new_turn_after_terminal {
                        log!(
                            "[AgentSession] A new turn started on a terminal-armed session for thread {}, re-arming emission",
                            thread_id
                        );
                        reset_per_turn_flags(
                            &mut is_waiting,
                            &mut last_emitted_idle,
                            &mut emitted_terminal_event,
                            &mut user_hit_stop,
                            &mut redirect_cause,
                            TurnTerminal {
                                kind: &mut last_terminal_kind,
                                withheld_api_error: &mut withheld_api_error,
                            },
                            &mut meta.actor,
                        );
                    }
                    announce_reads(
                        &self.event_bus,
                        thread_id,
                        &meta,
                        reads,
                        new_turn_after_terminal && read_owed_input,
                    )
                    .await;
                    let turn_ended = turn_has_terminal(emitted_terminal_event, &external_terminal_emitted);
                    match ev {
                        AgentEvent::Init { session_id: cc_sid, model: init_model, slash_commands: cmds, skills, agent_version } => {
                            log!("[AgentSession] [TIMING] Init event received: {:?}", cc_start.elapsed());
                            // Record what the backend actually attached to before
                            // anything else: the stale-resume veto reads it.
                            init_session_id = Some(cc_sid.clone());
                            // Enable --resume for follow-ups and engine restart
                            let cache_update = {
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) {
                                    s.backend_session_id = Some(cc_sid.clone());
                                    // Always update from Init: CC reports the full
                                    // model id, which is authoritative over any
                                    // alias the user selected.
                                    if let Some(ref m) = init_model {
                                        // Reconcile against the supplied alias so
                                        // the [1m] suffix survives. CC strips it,
                                        // and `context_window_for` keys on it.
                                        let norm = crate::runtime::claude_code::reconcile_cc_model(
                                            cc_model.as_deref().or(cc_default_model.as_deref()),
                                            m,
                                        );
                                        s.current_model = Some(norm.clone());
                                        normalized_model = Some(norm);
                                    }
                                    let skill_set: std::collections::HashSet<&str> = skills.iter().map(String::as_str).collect();
                                    s.builtin_commands = cmds.into_iter()
                                        .filter(|c: &String| !skill_set.contains(c.as_str()))
                                        .collect();
                                    s.skill_commands = skills;
                                    Some(s.to_commands_info())
                                } else {
                                    None
                                }
                            };
                            if coding_agent == CodingAgent::ClaudeCode {
                                self.refresh_cc_models_if_stale(agent_version.as_deref());
                            }
                            // Update per-repo cache outside sessions lock to avoid nested locks
                            if let Some(info) = cache_update {
                                let repo_key = repo_root.to_string_lossy().to_string();
                                self.upsert_cc_commands_cache(repo_key, info).await;
                            }
                            // Persist the session id and authoritative model the
                            // instant CC reports them at Init. Emitted even when
                            // the model is unchanged, so the session id is
                            // durable *before* the first `CodingAgentIdled`. A
                            // mid-turn restart can then still `--resume`, because
                            // the recovery lookups read `cc_session_id` from this
                            // event too. Without it, a long turn interrupted
                            // before its first idle loses the id entirely.
                            if let Err(e) = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentSettingsChanged {
                                    model: normalized_model.clone().filter(|_| records_rung),
                                    reasoning_effort: cc_reasoning_effort.clone().filter(|_| records_rung),
                                    coding_agent,
                                    cc_session_id: Some(cc_sid.clone()),
                                    // Pin the session-to-config-dir pairing at
                                    // Init. A later resume must re-inject this
                                    // dir to find the transcript.
                                    claude_config_dir: effective_pin.as_ref().map(|pin| pin.dir().to_string()),
                                    claude_config_dir_explicit: effective_pin.as_ref().map(|pin| pin.is_explicit()),
                                },
                                meta: meta.clone(),
                            }).await {
                                log!("[AgentSession] Failed to persist Init CodingAgentSettingsChanged for {}: {}", thread_id, e);
                            }
                            // The process is initialized and mostly idle now.
                            if let Some(permit) = startup_permit.take() {
                                drop(permit);
                                log!("[AgentSession] [TIMING] Startup semaphore released: {:?}", cc_start.elapsed());
                            }
                        }
                        // Straggler guard: the runtime sometimes delivers a
                        // trailing Message AFTER this turn's terminal event.
                        // Emitting it as `CodingAgentTextStreamed` would re-flip
                        // the projection to `running` and strand an already-idled
                        // thread there forever. Drop it. A real follow-up turn
                        // re-arms emission through `reset_per_turn_flags`.
                        AgentEvent::Message { text, .. } | AgentEvent::ProgressNote { text }
                            if turn_ended =>
                        {
                            log!(
                                "[AgentSession] Dropping post-terminal straggler text ({} chars) for thread {} — would resurrect 'running' on an idled thread",
                                text.len(),
                                thread_id
                            );
                        }
                        AgentEvent::Message { text, opens_block, .. } => {
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            // Reasoning precedes the answer, so flush any pending
                            // thought and let the "Thinking" step resolve first.
                            self.flush_coding_agent_thought(
                                thread_id,
                                &claude_thought_buf,
                                &mut last_thought_persisted_len,
                                coding_agent,
                                &meta,
                            )
                            .await;
                            if opens_block {
                                claude_text_buf.break_paragraph();
                            }
                            claude_text_buf.push(&text);
                            // Persist + broadcast at natural boundaries
                            if should_flush(claude_text_buf.as_str()) {
                                claude_text_buf.flush(&self.event_bus, thread_id, coding_agent, &meta, "[AgentSession] CodingAgentTextStreamed (Message flush)").await;
                            }
                        }
                        AgentEvent::ProgressNote { text } => {
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            self.flush_coding_agent_thought(
                                thread_id,
                                &claude_thought_buf,
                                &mut last_thought_persisted_len,
                                coding_agent,
                                &meta,
                            )
                            .await;
                            claude_text_buf
                                .flush_progress_note(&text, &self.event_bus, thread_id, coding_agent, &meta)
                                .await;
                        }
                        AgentEvent::SubAgentMessage { .. } if turn_ended => {}
                        // A sub-agent's prose is recorded whole, tagged with its
                        // `Agent` call, and kept out of the session's text buffer:
                        // that buffer is the session's own reply.
                        AgentEvent::SubAgentMessage { text, parent_tool_use_id } => {
                            let text = text.trim();
                            if !text.is_empty() {
                                self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed {
                                        text: text.to_string(),
                                        coding_agent,
                                        parent_tool_use_id: Some(parent_tool_use_id),
                                        progress_note: false,
                                    },
                                    meta: meta.clone(),
                                }, "[AgentSession] CodingAgentTextStreamed (sub-agent)").await;
                            }
                        }
                        // Reasoning stream, with the same straggler guard as
                        // Message and ToolUse. Its projection arm bumps
                        // `running`, so a thought arriving after the turn's
                        // terminal event would resurrect an idled thread.
                        AgentEvent::Thought { .. } if turn_ended => {}
                        AgentEvent::Thought { text } => {
                            // A thought is the first sign of life on a resumed
                            // turn, so clear the waiting state.
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            claude_thought_buf.push_str(&text);
                            // Flush on a paragraph boundary or once enough has
                            // piled up, so a long unbroken paragraph still
                            // streams live instead of landing at turn end.
                            if should_flush(&claude_thought_buf)
                                || claude_thought_buf.len() - last_thought_persisted_len
                                    >= THOUGHT_FLUSH_THRESHOLD
                            {
                                self.flush_coding_agent_thought(
                                    thread_id,
                                    &claude_thought_buf,
                                    &mut last_thought_persisted_len,
                                    coding_agent,
                                    &meta,
                                )
                                .await;
                            }
                        }
                        // Straggler guard, as in the Message arm above. Drop it
                        // without touching the in-flight counter, because a
                        // post-terminal turn has no live watchdog to disarm.
                        AgentEvent::ToolUse { name, id, input, .. } if turn_ended => {
                            // No text is left to persist, so a waiting card
                            // may take its second look.
                            if crate::runtime::is_user_question_tool(&name) {
                                question_flushes.0.flushed(&id, &name, &input);
                            }
                            log!(
                                "[AgentSession] Dropping post-terminal straggler tool call for thread {} — would resurrect 'running' on an idled thread",
                                thread_id
                            );
                        }
                        AgentEvent::ToolUse { name, input, id, parent_tool_use_id, api_call_id } => {
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            // Interleaved-thinking turns reason right up to a tool
                            // call, so flush before the tool step.
                            self.flush_coding_agent_thought(
                                thread_id,
                                &claude_thought_buf,
                                &mut last_thought_persisted_len,
                                coding_agent,
                                &meta,
                            )
                            .await;
                            claude_text_buf.flush(&self.event_bus, thread_id, coding_agent, &meta, "[AgentSession] CodingAgentTextStreamed (pre-ToolUse flush)").await;
                            claude_text_buf.break_paragraph();
                            // Disarm the watchdog while ANY tool runs, including
                            // AskUserQuestion: the user may take ten minutes to
                            // answer, and the session must survive that. The
                            // matching ToolResult arm decrements. `Relaxed` is
                            // fine, since the only reader tolerates one-tick
                            // staleness.
                            tools_in_flight.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            // A tool call this turn proves the session is alive,
                            // even for a terse model that emits no text. Gates
                            // the stale-resume heuristic below.
                            tool_calls_seen = tool_calls_seen.saturating_add(1);
                            tool_names.record(&id, &name);
                            if crate::runtime::is_user_question_tool(&name) {
                                // The text before the card is persisted above,
                                // so its gate may take its second look.
                                question_flushes.0.flushed(&id, &name, &input);
                                // Question flow: the subprocess blocks until the
                                // user answers, and the engine renders the card
                                // from the `UserQuestionAsked` the internal
                                // endpoint emits. Every route into that endpoint
                                // is named by `runtime::is_user_question_tool`.
                                // `run_session` has nothing to do here: no emit,
                                // which would double-surface the question, no
                                // kill, and no session removal.
                            } else {
                                // Safety net: a command can carry a token the
                                // agent read from its env, or a hardcoded
                                // database URI. So mask every string in `args`,
                                // with the tool-result mask, before the store.
                                //
                                // BEFORE the description, not after: the step row
                                // renders the description exactly as it renders
                                // the args, so both must be built from the
                                // redacted copy. `agentic_loop::run` orders it
                                // the same way.
                                let mut input = input;
                                crate::core::redact_subprocess_output_in_json(&mut input, &tool_result_secrets);
                                let description = crate::core::describe_cc_tool(&name, &input);
                                self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentToolCalled {
                                        name,
                                        args: input,
                                        description,
                                        coding_agent,
                                        tool_use_id: id,
                                        parent_tool_use_id,
                                        api_call_id,
                                    },
                                    meta: meta.clone(),
                                }, "[AgentSession] CodingAgentToolCalled").await;
                            }
                        }
                        // Straggler guard, as in the Message arm above. Still
                        // release the in-flight slot in case a pre-terminal
                        // ToolUse incremented it, then drop the event without
                        // emitting. `release_tool_slot` floors at 0, so an
                        // unpaired result is a no-op. Its call's name is
                        // dropped too, so a reused id cannot inherit it.
                        AgentEvent::ToolResult { id, .. } if turn_ended => {
                            release_tool_slot(&tools_in_flight);
                            tool_names.take(&id);
                            log!(
                                "[AgentSession] Dropping post-terminal straggler tool result for thread {} — would resurrect 'running' on an idled thread",
                                thread_id
                            );
                        }
                        AgentEvent::ToolResult { output, status: _, id, parent_tool_use_id } => {
                            // Re-arm the watchdog if this was the last in-flight
                            // tool. Floored at 0 so an unpaired ToolResult cannot
                            // underflow (see `release_tool_slot`).
                            release_tool_slot(&tools_in_flight);
                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentToolResult {
                                    name: tool_names.take(&id),
                                    result: super::super::tool_output::stored_tool_output(&output, &tool_result_secrets),
                                    coding_agent,
                                    tool_use_id: id,
                                    parent_tool_use_id,
                                },
                                meta: meta.clone(),
                            }, "[AgentSession] CodingAgentToolResult").await;
                        }
                        AgentEvent::Usage {
                            model: cc_msg_model,
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_creation_tokens,
                            cache_creation_1h_tokens,
                            parent_tool_use_id,
                            api_call_id,
                            duration_ms,
                            reasoning_effort,
                        } => {
                            // A usage frame reaches us only for a real API call
                            // (the parser drops all-zero ones), so this is the
                            // proof-of-model-call the resume-settle skip reads.
                            api_calls_seen = api_calls_seen.saturating_add(1);
                            self.record_coding_agent_usage(
                                thread_id,
                                &external_terminal_emitted,
                                &meta,
                                coding_agent,
                                CallModels::of(normalized_model.as_deref(), cc_msg_model.as_deref()),
                                ReportedUsage {
                                    input_tokens,
                                    output_tokens,
                                    cache_read_tokens,
                                    cache_creation_tokens,
                                    cache_creation_1h_tokens,
                                    parent_tool_use_id,
                                    api_call_id,
                                    duration_ms,
                                    reasoning_effort,
                                },
                                cc_reasoning_effort.as_deref(),
                            )
                            .await;
                        }
                        AgentEvent::Compacted { trigger, pre_tokens, post_tokens } => {
                            // The summary may have dropped the device block.
                            self.sent_device_contexts.forget(thread_id);
                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentCompacted {
                                    trigger,
                                    pre_tokens,
                                    post_tokens,
                                    coding_agent,
                                },
                                meta: meta.clone(),
                            }, "[AgentSession] CodingAgentCompacted").await;
                        }
                        AgentEvent::OutputEnded => unreachable!("OutputEnded skipped above"),
                        // Liveness-only ping from a streaming delta. The
                        // top-of-loop heartbeat bump already recorded that the
                        // subprocess is alive. That is what stops the watchdog
                        // from killing a long single step that streams past
                        // `WATCHDOG_INACTIVITY_LIMIT_MS` without finishing.
                        // Nothing to persist: the complete text or tool call
                        // arrives separately.
                        AgentEvent::StreamActivity => {}
                        // The ledger accounted for both at the top of this arm.
                        // A block counts as sent once the agent has read it.
                        AgentEvent::InputRead { input } => {
                            let block = blocks_in_flight.remove(&input);
                            if let (true, Some(fingerprint), Some(conversation)) =
                                (read_owed_input, block, init_session_id.as_deref())
                            {
                                self.sent_device_contexts.record(thread_id, conversation, fingerprint);
                            }
                        }
                        AgentEvent::InputRejected { input } => {
                            blocks_in_flight.remove(&input);
                        }
                        AgentEvent::Exited { .. } => unreachable!("Exited handled above"),
                        AgentEvent::Result { text, error: cc_error, .. } => {
                                        let err_suffix = cc_error.as_deref().map(|e| format!(" (error: {})", e)).unwrap_or_default();
                                        log!("[AgentSession] Result event received — entering waiting state{}", err_suffix);
                                        // Final flush of any pending reasoning: a
                                        // turn that reasoned then ended without
                                        // text, or the tail below the threshold.
                                        self.flush_coding_agent_thought(
                                            thread_id,
                                            &claude_thought_buf,
                                            &mut last_thought_persisted_len,
                                            coding_agent,
                                            &meta,
                                        )
                                        .await;
                                        // After an external terminal, text in
                                        // `Result.text` beyond the buffer is the
                                        // agent's reply to the teardown
                                        // interrupt, which the Message guard
                                        // dropped. Only the external half counts:
                                        // a command sent mid-turn answers after
                                        // the loop's own terminal.
                                        let result_text_is_live = !external_terminal_emitted
                                            .load(std::sync::atomic::Ordering::Acquire);
                                        // Final flush of any pending text
                                        if !claude_text_buf.is_empty() {
                                            // `Result.text` may carry text beyond
                                            // what the Message events streamed.
                                            // Append the extra, so the frontend
                                            // sees the complete text before the
                                            // session goes to waiting. Mirrors
                                            // `build_session_messages_with`.
                                            let result_trimmed = text.trim();
                                            let extra = result_trimmed
                                                .strip_prefix(claude_text_buf.as_str().trim())
                                                .map(str::trim)
                                                .filter(|extra| result_text_is_live && !extra.is_empty());
                                            if let Some(extra) = extra {
                                                claude_text_buf.break_paragraph();
                                                claude_text_buf.push(extra);
                                            }
                                            claude_text_buf.flush(&self.event_bus, thread_id, coding_agent, &meta, "[AgentSession] CodingAgentTextStreamed (Result flush)").await;
                                        } else if result_text_is_live
                                            && result_text_is_own_prose(&text, cc_error.as_deref())
                                        {
                                            // A slash command produces a Result
                                            // with no preceding Message events,
                                            // so emit its text for the frontend.
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: text.trim().to_string(), coding_agent, parent_tool_use_id: None, progress_note: false },
                                                meta: meta.clone(),
                                            }, "[AgentSession] CodingAgentTextStreamed (slash command result)").await;
                                        }
                                        // Both buffers must be checked: the slash-command path emits
                                        // Result.text without buffering it, so `claude_text_buf` alone
                                        // would mis-flag /model output as empty.
                                        let result_text_empty = text.trim().is_empty();
                                        let buffered_text_empty = claude_text_buf.as_str().trim().is_empty();
                                        // Detect a stale resume: the backend
                                        // returned an empty Result right after
                                        // resuming an expired session. Abort with
                                        // no terminal and no idle, so the caller
                                        // can retry against a fresh session.
                                        //
                                        // Two independent signals both mean "the
                                        // id we resumed is gone": the empty-echo
                                        // heuristic, and CC's explicit
                                        // session-not-found error. The explicit
                                        // one arrives as a `cc_error` and so
                                        // bypasses the heuristic's gate, but it
                                        // is deterministic, which is why it is
                                        // whitelisted. Both take the same path:
                                        // shadow the dead id, keep the branch,
                                        // and return `STALE_RESUME_ERROR`.
                                        let explicit_session_not_found = resume_session_id.is_some()
                                            && is_definitive_session_not_found(cc_error.as_deref());
                                        // Did the backend attach to the
                                        // conversation we asked for? Its Init
                                        // echoes the id it actually opened, and a
                                        // FAILED resume yields a different one.
                                        // So a match is structural proof of life
                                        // and vetoes the output-shape heuristic.
                                        let stale_inputs = StaleResumeInputs {
                                            has_resume_session: resume_session_id.is_some(),
                                            resume_attach_confirmed: resume_session_id.is_some()
                                                && init_session_id.as_deref()
                                                    == resume_session_id.as_deref(),
                                            result_text_empty,
                                            buffered_text_empty,
                                            no_prior_results_this_turn: result_texts.is_empty(),
                                            no_tool_calls_this_turn: tool_calls_seen == 0,
                                            cc_error: cc_error.is_some(),
                                        };
                                        // The turn's SHAPE said "stale", the
                                        // confirmed attach overruled it, and no
                                        // API call was made. So this Result
                                        // closes the backend's own resume-settle
                                        // turn, not ours. Our prompt may not even
                                        // be dequeued yet. Ending the turn here
                                        // would report a failure that did not
                                        // happen and kill the subprocess
                                        // mid-answer. Skip it and keep reading.
                                        if is_resume_settle_result(stale_inputs, api_calls_seen == 0) {
                                            log!(
                                                "[AgentSession] thread {} resumed sid={} and the backend confirmed the attach, but this Result carries no text, no tool call and no error: it closes the resume-settle turn, not ours. Skipping it and waiting for the real Result.",
                                                thread_id,
                                                resume_session_id.as_deref().unwrap_or("")
                                            );
                                            // Record it so the skip cannot repeat:
                                            // `no_prior_results_this_turn` goes false, so
                                            // any further Result classifies normally.
                                            result_texts.push(text.clone());
                                            continue 'event_loop;
                                        }
                                        // Same empty shape and same confirmed
                                        // attach, but the backend DID call the
                                        // model, so what came back answers our
                                        // prompt however empty. It classifies
                                        // below. Logged because a wrong skip here
                                        // strands the turn at `running`. This
                                        // line says the settle skip considered
                                        // the turn, and the API call ruled it
                                        // out.
                                        if api_calls_seen > 0
                                            && is_resume_settle_result(stale_inputs, true)
                                        {
                                            log!(
                                                "[AgentSession] thread {} resumed sid={} and produced an empty Result with no tool calls, but {} API call(s) were made: it answers our prompt, not a resume-settle turn. Classifying it.",
                                                thread_id,
                                                resume_session_id.as_deref().unwrap_or(""),
                                                api_calls_seen
                                            );
                                        }
                                        if is_stale_resume_signal(stale_inputs)
                                            || explicit_session_not_found
                                        {
                                            log!("[AgentSession] Stale resume detected (empty-echo heuristic or explicit session-not-found) — aborting session to retry with a fresh spawn.");
                                            agent_cancel.cancel();
                                            // The entry goes back to `Starting`: the
                                            // caller's retry runs on this claim, so
                                            // the thread stays held across the gap.
                                            {
                                                let mut guard = self.agent_sessions.lock().await;
                                                idle_notify.notify_waiters();
                                                if let Some(s) = guard.get_mut(&thread_id).filter(|s| {
                                                    std::sync::Arc::ptr_eq(&s.external_terminal_emitted, &external_terminal_emitted)
                                                }) {
                                                    s.phase = crate::engine::SessionPhase::Starting {
                                                        startup_cancel: startup_cancel.clone(),
                                                    };
                                                    s.is_waiting = false;
                                                }
                                            }
                                            claim.entry_guard.reap_only();
                                            // Shadow the stale `CodingAgentIdled`,
                                            // so `resolve_resume_context` cannot
                                            // reuse the dead id on the retry.
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::SessionEnded {
                                                    reason: SessionEndReason::StaleResume,
                                                },
                                                meta: meta.clone(),
                                            }, "[AgentSession] SessionEnded (stale resume)").await;
                                            // NEVER CREATE A DUPLICATE: await the
                                            // old process group's full teardown
                                            // before signalling the retry. The
                                            // driver drops `events_tx` only after
                                            // the kill completes. So draining
                                            // `events_rx` to a closed channel is
                                            // a precise await of "the old process
                                            // group is dead". Without it the
                                            // retry can spawn while the wedged
                                            // old process still holds the SHARED
                                            // worktree. Bounded, so a stuck
                                            // teardown cannot wedge the loop.
                                            await_process_teardown(&mut events_rx, thread_id, "a stale resume").await;
                                            // NEVER DELETE A POSSIBLY-USEFUL
                                            // WORKTREE. The deterministic
                                            // `thread-<id>` worktree is SHARED
                                            // per thread and holds the user's
                                            // warm working copy. The retry reuses
                                            // it: `spawn_context` runs
                                            // `worktree_add` only when the dir is
                                            // absent. The branch is the thread's
                                            // durable anchor and is kept
                                            // regardless, and the cleanup worker
                                            // is the sole deleter of a valid
                                            // worktree (ADR 0035).
                                            return Err(STALE_RESUME_ERROR.into());
                                        }

                                        result_texts.push(text.clone());
                                        // Single read of `shutting_down`, so the
                                        // terminal-event and skip-idle decisions
                                        // agree. Widened through
                                        // `session_is_shutting_down`: the
                                        // per-session flag alone misses a session
                                        // registered after the teardown snapshot,
                                        // which would read a restart as a Stop.
                                        let is_shutdown = self.session_is_shutting_down(
                                            shutting_down
                                                .load(std::sync::atomic::Ordering::Relaxed),
                                        );
                                        // A quiet pick ends the turn with no
                                        // reply by design, so its empty Result
                                        // is not the killed-tool failure.
                                        let ended_on_quiet_answer = self
                                            .agent_sessions
                                            .lock()
                                            .await
                                            .get_mut(&thread_id)
                                            .is_some_and(|s| std::mem::take(&mut s.quiet_answer_pending));
                                        let (terminal_kind, emit_idle) = classify_result(
                                            user_hit_stop,
                                            redirect_cause,
                                            is_shutdown,
                                            cc_error,
                                            buffered_text_empty && result_text_empty && !ended_on_quiet_answer,
                                        );
                                        // Capture before the terminal emit below moves
                                        // it out. The idle gate and the post-loop
                                        // cleanup both read it to refuse partial
                                        // work.
                                        last_terminal_kind = Some(terminal_kind.clone());
                                        // Decide the auto-resume BEFORE emitting:
                                        // the emit is what tells the parent, and
                                        // the exit that resumes runs long after.
                                        // Released below, once both emits are
                                        // out. See ADR 0199.
                                        self.hold_completion_if_api_error_resume(
                                            thread_id,
                                            &last_terminal_kind,
                                            is_shutdown,
                                            is_resolution_session,
                                        )
                                        .await;
                                        // A Result is a turn boundary, so both
                                        // the user-stop latch and the cancel
                                        // actor on `meta` must clear here. An
                                        // interrupt kept alive by inflight
                                        // follow-ups would otherwise relabel
                                        // their success as a second Canceled,
                                        // and a resumed turn would inherit the
                                        // cancelling device.
                                        let clears = terminal_clears_user_hit_stop(&terminal_kind);
                                        if clears {
                                            user_hit_stop = false;
                                            redirect_cause = None;
                                        }
                                        if !crate::engine::agent_session::runtime_helpers::external_terminal_already_emitted(&self.pool, &external_terminal_emitted, thread_id, meta.request_event_id, is_shutdown, "Result classify").await {
                                            let (model, reasoning_effort) = self
                                                .terminal_model_settings(thread_id, &external_terminal_emitted)
                                                .await;
                                            let terminal_event = Self::make_terminal_event(
                                                terminal_kind,
                                                text.clone(),
                                                model,
                                                reasoning_effort,
                                            );
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: terminal_event,
                                                meta: meta.clone(),
                                            }, "[AgentSession] terminal event (Result classify)").await;
                                        }
                                        // Clear AFTER the emit so the cancel
                                        // terminal still carries the device.
                                        if clears {
                                            meta.actor = None;
                                        }
                                        emitted_terminal_event = true;
                                        // A Result landed, so CC honored the
                                        // interrupt. Disarm the bounded Esc
                                        // fallback before it escalates.
                                        interrupt_escalate_at = None;
                                        claude_text_buf.clear();
                                        claude_thought_buf.clear();
                                        last_thought_persisted_len = 0;
                                        // Auto-commit dirty files before checking
                                        // for changes, unless a restart ended the
                                        // turn. See `result_auto_commits`.
                                        // A keep-alive turn changes nothing: no
                                        // commit, proposal, read decision or nudge.
                                        let keep_alive_turn = self.event_bus.in_keep_alive_turn(thread_id);
                                        let commits = !keep_alive_turn && last_terminal_kind.as_ref().is_some_and(result_auto_commits);
                                        if let Some(wt) = worktree_path.as_ref().filter(|_| commits) {
                                            auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Coding agent changes (auto-committed)").await;
                                        }
                                        // Resolve the branch and probe the diff
                                        // ONCE, together, in
                                        // `idle_change_state`. The spawn-time
                                        // `branch_name` goes stale as soon as a
                                        // repo skill renames the branch. A
                                        // repeated probe also lets one git
                                        // failure reach several consumers as
                                        // "there is no diff". This site WRITES
                                        // its answer somewhere durable, so it
                                        // gets git truth or an explicit unknown,
                                        // never a silent no.
                                        let (wt_has_changes, wt_requires_restart, changed_files) = if is_resolution_session {
                                            (true, false, None) // A resolution session always has work
                                        } else {
                                            let state = resolve_idle_change_state(IdleChangeStateInput {
                                                pool: self.pool(),
                                                thread_id,
                                                repo_root: &repo_root,
                                                worktree_path: worktree_path.as_deref(),
                                                tracked_branch: &branch_name,
                                                is_external_repo,
                                                anchor_sha: adoption_anchor_sha.as_deref(),
                                            })
                                            .await;
                                            // One name per session from here on: the propose, the
                                            // session-state write-back below, and the post-loop cleanup
                                            // all follow the adoption.
                                            branch_name = state.branch_name;
                                            (state.has_changes, state.requires_restart, state.changed_files)
                                        };

                                        is_waiting = true;
                                        {
                                            let mut sessions = self.agent_sessions.lock().await;
                                            if let Some(s) = sessions.get_mut(&thread_id) {
                                                crate::engine::agent_session::lifecycle::mark_turn_boundary(s);
                                                // Keep ONE branch name per session. An
                                                // adoption above moved the run loop onto
                                                // the worktree's real branch. Without this
                                                // write-back, `apply_now` and the stop and
                                                // discard paths keep acting on the stale
                                                // name.
                                                s.branch_name = Some(branch_name.clone());
                                                // The follow-up redirect waits for this
                                                // boundary.
                                                s.idle_notify.notify_waiters();
                                            }
                                        }
                                        // A prompt waiting on its own turn is
                                        // answered here, and only by the turn
                                        // that read it.
                                        inputs.turn_ended();
                                        // Propose the change at idle time so the Apply button
                                        // shows immediately (propose_change deduplicates). When
                                        // CC skipped /harden, hardened=false propagates to the
                                        // change record and Apply runs hardening at click time.
                                        // A live event wait holds a clean idle's proposal. See
                                        // `idle_change_write` for that and the other guards.
                                        //
                                        // Before the idle emit, never after it: the idle
                                        // builds the parent's completion card from the
                                        // changes as they stand then. Pinned by
                                        // `the_idle_arm_proposes_before_it_idles`.
                                        //
                                        // The gate deliberately omits a `wt_has_changes`
                                        // term, so the empty-diff arm below is reachable.
                                        // A branch whose diff cancelled out still has a
                                        // pending row to reconcile, which would otherwise
                                        // keep claiming files the branch no longer has.
                                        let mut held_proposal = None;
                                        let holds_live_wait = !self.live_waits.for_thread(thread_id).await.is_empty();
                                        let idle_write = idle_change_write(
                                            IdleSession {
                                                is_external_repo,
                                                is_shutdown,
                                                is_resolution_session,
                                                holds_live_wait,
                                            },
                                            &last_terminal_kind,
                                        )
                                        .filter(|_| !keep_alive_turn);
                                        if let Some(write) = idle_write {
                                            // Reuses the single probe above rather than re-asking git.
                                            // `None` means the probe could not be answered, and an
                                            // unanswered probe must never touch durable change state:
                                            // proposing would need a file list we don't have, and
                                            // reconciling would zero a pending row's files on a git
                                            // hiccup. Both wait for the next idle.
                                            match changed_files.as_deref() {
                                                None => {
                                                    log!(
                                                        "[AgentSession] Skipping propose/reconcile at idle for thread {}: the diff probe could not be answered",
                                                        thread_id
                                                    );
                                                }
                                                Some([]) => {
                                                    // No committed diff against the base.
                                                    // Never auto-discard an existing
                                                    // pending row, because the user
                                                    // resolves it from Review. Do re-sync
                                                    // it to zero files, so the card stops
                                                    // advertising work the branch no
                                                    // longer carries.
                                                    self.reconcile_emptied_pending_change(thread_id, &repo_root, &branch_name).await;
                                                }
                                                Some(_) if !write.may_create() && !self.branch_has_open_change(&branch_name).await => {
                                                    log!(
                                                        "[AgentSession] Holding the idle proposal for thread {}: it holds a live event wait",
                                                        thread_id
                                                    );
                                                }
                                                Some(changed_files) => {
                                                    // Only the propose path needs the marker: the
                                                    // reconcile above re-reads it itself, and only if
                                                    // it actually has a row to correct.
                                                    let hardened = has_full_harden_marker(&self.pool, &repo_root, &branch_name).await;
                                                    let requires_restart = files_require_restart(changed_files);
                                                    let fallback = change_description_fallback(self.pool(), thread_id, &branch_name).await;
                                                    let base = default_local_branch(&repo_root).await;
                                                    let log_range = format!("{}..{}", base, branch_name);
                                                    let description = describe_branch_changes(&repo_root, &log_range, &fallback, None).await;
                                                    let repo_root_str = repo_root.to_string_lossy().to_string();
                                                    match self.propose_turn_work(crate::engine::change_ops::ProposeChangeInput {
                                                        thread_id,
                                                        branch_name: &branch_name,
                                                        repo_root: &repo_root_str,
                                                        description: &description,
                                                        files: changed_files,
                                                        requires_restart,
                                                        channel: EventChannel::ClaudeCode,
                                                        hardened,
                                                        // Live agent proposal: origin is
                                                        // carried by the surrounding
                                                        // MessageReceived. Engine-internal
                                                        // recovery paths stamp Engine origin
                                                        // via propose_branch_changes.
                                                        origin: None,
                                                    }, write.finished()).await {
                                                        Ok(ProposeOutcome::Proposed(_)) => {
                                                            // Track for the ProcessResult returned via the
                                                            // Exited arm. Every idle exits the subprocess,
                                                            // so the post-loop cleanup that used to set
                                                            // this is skipped now.
                                                            proposed_change = true;
                                                        }
                                                        Ok(ProposeOutcome::Held(hold)) => {
                                                            held_proposal = Some(hold);
                                                        }
                                                        Ok(ProposeOutcome::Unfinished | ProposeOutcome::AwaitingLanding(_)) => {}
                                                        Err(e) => {
                                                            log!("[AgentSession] Failed to propose change at idle: {}", e);
                                                        }
                                                    }
                                                    self.broadcast_changes_updated().await;
                                                }
                                            }
                                        }

                                        // Recorded on the `CodingAgentIdled` payload for
                                        // the event history. It gates nothing: a running
                                        // background task re-opens the thread through its
                                        // event wait. A task the thread already stopped
                                        // re-opens nothing, so it does not count.
                                        let bg_bash_running = self
                                            .bash_background
                                            .running_for_thread(thread_id)
                                            .await
                                            .iter()
                                            .any(|task| !task.stop_requested);

                                        // Shutdown: emitting idle would make recover_orphaned_worktrees
                                        // skip this session as "truly idle" and break recovery.
                                        if emit_idle {
                                            // The turn-end gate's forced decisions, before
                                            // the idle records the turn's end (ADR 0417). An
                                            // engine-run merge resolution is not the user's
                                            // turn, and an unfinished turn decides nothing.
                                            if !is_resolution_session
                                                && !keep_alive_turn
                                                && matches!(last_terminal_kind, Some(TerminalKind::Generated))
                                            {
                                                self.gate_coding_agent_turn_end(thread_id, wt_has_changes).await;
                                            }
                                            self.emit_coding_agent_idled(
                                                thread_id,
                                                CodingAgentIdleSnapshot {
                                                    has_changes: wt_has_changes,
                                                    is_external_repo,
                                                    requires_restart: wt_requires_restart,
                                                    bg_bash_pending: bg_bash_running,
                                                    worktree_path: worktree_path.as_deref(),
                                                    keep_alive: keep_alive_turn,
                                                },
                                                &meta,
                                                coding_agent,
                                            ).await;
                                            last_emitted_idle = true;
                                            // Cleared before the run lets go of
                                            // the thread, so the real turn
                                            // waiting on it projects in full.
                                            if keep_alive_turn {
                                                self.event_bus.mark_keep_alive_turn(thread_id, false).await;
                                            }
                                        }
                                        // Both fan-in-visible events for this
                                        // terminal are out, so the hold has done
                                        // its whole job. Releasing HERE rather
                                        // than at the resume site is what keeps a
                                        // hold from outliving the terminal it
                                        // names. A `KeepAlive` below can carry
                                        // this loop through another turn, and a
                                        // Stop or a safety net inside that turn
                                        // must report normally. What comes back
                                        // carries the decision on from here.
                                        withheld_api_error =
                                            self.event_bus.auto_resume_holds().release(thread_id);

                                        // After the idle, so a re-entry opens its own
                                        // exchange. Before `terminate_decision`, which
                                        // then sees it queued and keeps the subprocess.
                                        if let Some(hold) = held_proposal {
                                            let gate = crate::engine::turn_end::TurnEndState::at_session_idle(
                                                true,
                                                idle_write == Some(IdleChangeWrite::Complete),
                                                crate::engine::turn_end::SessionFacts {
                                                    head: branch_head_sha(&repo_root, &branch_name).await,
                                                    held: Some(hold),
                                                    handoff_pending: crate::engine::successor::handoff_pending(self.pool(), thread_id).await,
                                                },
                                            );
                                            if let Some(hold) = reentry_ledger.take_due(&gate) {
                                                if let Err(e) = self.nudge_held_proposal_in_session(thread_id, hold, &msg_tx).await {
                                                    log!("[AgentSession] Proposal-hold re-entry for thread {} not sent: {}", thread_id, e);
                                                }
                                            }
                                        }

                                        match idle_action(is_resolution_session, is_shutdown) {
                                            IdleAction::EndSession => {
                                                // The rule an ordinary session exits by: a
                                                // follow-up on its way gets its turn first.
                                                if self.exit_idle_subprocess_unless_owed(thread_id, msg_rx, inputs.owed(), &agent_cancel, &mut handed_off).await {
                                                    log!("[AgentSession] Resolution session idle for thread {}: ending loop", thread_id);
                                                    break 'event_loop;
                                                }
                                            }
                                            IdleAction::ExitSubprocess => {
                                                self.exit_idle_subprocess_unless_owed(thread_id, msg_rx, inputs.owed(), &agent_cancel, &mut handed_off).await;
                                            }
                                            IdleAction::Nothing => {}
                                        }
                                    }
                                }
                }

                // Codex app-server approval bridge. Each request spawns its own
                // waiter task, so this loop NEVER blocks on the user. A pending
                // card must not stall event processing, interrupts or the
                // watchdog, so keep this arm minimal.
                perm_req = async {
                    match agent_permission_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        // No in-band permission channel, so pend forever and
                        // let the arm never fire.
                        None => std::future::pending().await,
                    }
                } => {
                    match perm_req {
                        Some(req) => {
                            let pool = self.pool.clone();
                            let bus = self.event_bus.clone();
                            let pending = self.pending_cc_permission.clone();
                            // An unattended session auto-resolves the card from
                            // the inherited side-effect grant rather than
                            // hanging, and that decision needs the trigger
                            // registry and the workspace root.
                            let trigger_configs = self.trigger_configs.clone();
                            let workspace_path = self.workspace_path.clone();
                            // A file write inside this session's own worktree
                            // skips the card entirely. This local IS what seeded
                            // `AgentSession.worktree_path`, so it matches what
                            // the MCP path resolves, and reading it here keeps
                            // the arm lock-free.
                            let session_worktree = worktree_path.clone();
                            // The attended command gate's LLM half runs off the
                            // engine, inside the spawned task below. Taken as an
                            // Arc for the same reason the title task does, and
                            // the select arm stays free of the classification.
                            // `try_` so a bare test engine gets the static half
                            // rather than a panic.
                            let judge = self.try_clone_arc();
                            // Disarm the watchdog while the card waits. The
                            // approval may arrive BEFORE the item's ToolUse,
                            // so the paired tool counter alone cannot cover
                            // the wait.
                            let tools = tools_in_flight.clone();
                            tokio::spawn(async move {
                                tools.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let mut respond = req.respond;
                                tokio::select! {
                                    outcome = crate::engine::cc_permission::prompt_coding_agent_permission(
                                        &pool,
                                        &bus,
                                        &pending,
                                        &trigger_configs,
                                        &workspace_path,
                                        session_worktree.as_deref(),
                                        judge.as_deref(),
                                        crate::engine::cc_permission::CodingAgentPermissionInput {
                                            thread_id,
                                            tool_use_id: req.id,
                                            tool_name: req.tool_name,
                                            input: req.input,
                                        },
                                    ) => {
                                        // Send failure = driver died between answer
                                        // and delivery; nothing to deliver to.
                                        let _ = respond.send(outcome.allowed);
                                    }
                                    // The driver died while the card was pending.
                                    // Its JoinSet aborts the waiter task, which
                                    // drops the oneshot receiver and fires this
                                    // arm. Abandoning the wait drops our
                                    // broadcast receiver, so the next prompt's
                                    // sweep evicts the entry. The persisted card
                                    // resolves through the same recovery paths
                                    // Claude Code uses.
                                    _ = respond.closed() => {
                                        log!(
                                            "[AgentSession] agent died with permission card pending for thread {} — abandoning waiter",
                                            thread_id
                                        );
                                    }
                                }
                                release_tool_slot(&tools);
                            });
                        }
                        None => {
                            // The driver dropped its sender, so disable the arm.
                            // A closed channel would re-resolve every tick.
                            agent_permission_rx = None;
                        }
                    }
                }

                Some(mut user_input) = msg_rx.recv(), if !handed_off => {
                    // After a handoff, input waits in the channel for the
                    // successor session. The idle exit drains it there.
                    if crate::engine::successor::handoff_pending(self.pool(), thread_id).await {
                        handed_off = true;
                        let _sessions = self.agent_sessions.lock().await;
                        requeue_at_head(&msg_tx, msg_rx, user_input);
                        continue 'event_loop;
                    }
                    // The input just left the channel and is about to reach the
                    // driver. It therefore moves from the "sent, not yet
                    // forwarded" window into the "forwarded, not yet read" one.
                    // Recording in this arm rather than at each `msg_tx.send`
                    // keeps the two windows disjoint, and it catches every
                    // sender. The record waits for the text the agent is sent.
                    reset_per_turn_flags(
                        &mut is_waiting,
                        &mut last_emitted_idle,
                        &mut emitted_terminal_event,
                        &mut user_hit_stop,
                        &mut redirect_cause,
                        TurnTerminal {
                            kind: &mut last_terminal_kind,
                            withheld_api_error: &mut withheld_api_error,
                        },
                        &mut meta.actor,
                    );
                    {
                        let mut sessions = self.agent_sessions.lock().await;
                        if let Some(s) = sessions.get_mut(&thread_id) {
                            s.is_waiting = false;
                            if let Some(id) = user_input.origin_event_id {
                                s.unforwarded_inputs.remove(&id);
                            }
                        }
                    }
                    // Flush any pending reasoning before the new user input opens a
                    // fresh turn, then reset so the next turn's thinking starts clean.
                    self.flush_coding_agent_thought(
                        thread_id,
                        &claude_thought_buf,
                        &mut last_thought_persisted_len,
                        coding_agent,
                        &meta,
                    )
                    .await;
                    claude_thought_buf.clear();
                    last_thought_persisted_len = 0;
                    claude_text_buf.flush(&self.event_bus, thread_id, coding_agent, &meta, "[AgentSession] CodingAgentTextStreamed (flush before user_input)").await;
                    claude_text_buf.clear();

                    let images = user_input.images.clone().unwrap_or_default();
                    let child_report_card = match (user_input.kind, user_input.origin_event_id) {
                        (crate::engine::AgentInputKind::ReentryFromEngine, Some(anchor)) => {
                            anchor_is_child_report(&self.pool, anchor).await.then_some(anchor)
                        }
                        _ => None,
                    };
                    // Only some kinds write this, see `writes_prompt_sent`; a
                    // child wake's own `ChildThreadCompleted` is the start. It
                    // goes out BEFORE the forward because the turn-gap note ends
                    // at it. It claims no delivery: it bounds the next note only
                    // once the agent's read names it (`turn_gap`).
                    let prompt_sent = if user_input.kind.writes_prompt_sent() {
                        self
                            .event_bus
                            .emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                                    input_event_id: user_input.origin_event_id,
                                    text: user_input.text.clone(),
                                    coding_agent,
                                    // The MessageReceived the API boundary
                                    // emitted carries the origin.
                                    origin: None,
                                },
                                meta: meta.clone(),
                            })
                            .await
                            .unwrap_or_else(|e| {
                                log!("[EventBus] [AgentSession] CodingAgentPromptSent emit failed: {}", e);
                                None
                            })
                            .map(|emitted| emitted.event_id)
                    } else {
                        None
                    };
                    blocks_in_flight.retain(|uuid, _| inputs.owes(*uuid));
                    let unread: Vec<ContextFingerprint> = blocks_in_flight.values().cloned().collect();
                    let follow_up = follow_up_for_agent(
                        &self.pool,
                        &self.event_store,
                        thread_id,
                        &user_input,
                        prompt_sent,
                        Some(branch_name.as_str()),
                        HeldBlocks::for_agent(
                            &self.sent_device_contexts,
                            coding_agent,
                            init_session_id.as_deref(),
                            &unread,
                        ),
                    )
                    .await;
                    let agent_input = AgentInput {
                        text: follow_up.text,
                        images,
                        uuid: Uuid::new_v4(),
                    };
                    if let Some(fingerprint) = follow_up.device_block {
                        blocks_in_flight.insert(agent_input.uuid, fingerprint);
                    }
                    match child_report_card {
                        Some(card_id) => inputs.forwarded_child_report(
                            card_id,
                            &agent_input,
                            user_input.completion.take(),
                            user_input.text.clone(),
                        ),
                        None => inputs.forwarded(
                            owed_input_event_ids(user_input.origin_event_id, prompt_sent),
                            &agent_input,
                            user_input.completion.take(),
                        ),
                    }
                    child_reports.publish(&inputs);
                    if agent_input_tx.send(agent_input).is_err() {
                        log!("[AgentSession] Failed to forward user input to agent runtime — channel closed");
                        break;
                    }
                    if let Some(id) = user_input.origin_event_id {
                        let (now_forwarded, still_waiting) = std::mem::take(&mut withdraws_awaiting_forward)
                            .into_iter()
                            .partition(|w| w.input_event_id == id);
                        withdraws_awaiting_forward = still_waiting;
                        for withdraw in now_forwarded {
                            begin_withdraw(&inputs, &agent_withdraw_tx, &self.pool, thread_id, withdraw, &mut pending_withdraws);
                        }
                    }
                }

                Some(withdraw) = withdraw_rx.recv() => {
                    // Read under the lock the chat fast path writes under, so
                    // a follow-up still in `msg_rx` is never mistaken for read.
                    let on_its_way = self
                        .agent_sessions
                        .lock()
                        .await
                        .get(&thread_id)
                        .is_some_and(|s| s.unforwarded_inputs.contains(&withdraw.input_event_id));
                    if on_its_way {
                        withdraws_awaiting_forward.push(withdraw);
                    } else {
                        begin_withdraw(&inputs, &agent_withdraw_tx, &self.pool, thread_id, withdraw, &mut pending_withdraws);
                    }
                }

                Some(take) = child_reports.take_rx.recv() => {
                    begin_take(&inputs, &agent_withdraw_tx, child_reports, &self.pool, thread_id, take, &mut pending_withdraws);
                }

                Some(withdrawn) = child_reports.settled_rx.recv() => {
                    for (withdrawn, resent) in settle_take(&self.event_bus, thread_id, &mut inputs, &agent_input_tx, withdrawn).await {
                        if let Some(block) = blocks_in_flight.remove(&withdrawn) {
                            blocks_in_flight.insert(resent, block);
                        }
                    }
                    child_reports.publish(&inputs);
                    let action = idle_action(is_resolution_session, self.session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed)));
                    if is_waiting && self.idle_exit_ends_session(thread_id, msg_rx, inputs.owed(), &agent_cancel, action, &mut handed_off).await {
                        break;
                    }
                }

                Some(answered) = pending_withdraws.next() => {
                    finish_withdraw(&self.event_bus, thread_id, &mut inputs, answered).await;
                    child_reports.publish(&inputs);
                    let action = idle_action(is_resolution_session, self.session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed)));
                    if is_waiting && self.idle_exit_ends_session(thread_id, msg_rx, inputs.owed(), &agent_cancel, action, &mut handed_off).await {
                        break;
                    }
                }

                _ = interrupt.notified() => {
                    // Cancel button = Esc in Claude Code CLI.
                    // Sends control_request:interrupt → CC stops current work, emits
                    // a Result, and goes idle. We set user_hit_stop so the Result
                    // handler emits ResponseCanceled (→ exchange "Canceled") instead
                    // of ResponseGenerated. CodingAgentIdled still follows, keeping
                    // the thread in "Waiting" state (CC is alive).
                    // During shutdown, the post-loop cleanup bails out early
                    // (no worktree removal, no SessionEnded). The session resumes
                    // after restart via recover_orphaned_worktrees.
                    if !is_waiting {
                        user_hit_stop = true;
                        // Distinguish a redirect (a follow-up, an effort
                        // restart) from a real Stop click: the former carries
                        // its own neutral cause, the latter is UserStop
                        // ("Canceled ✕"). Drained on read; cleared at the Result
                        // turn boundary below alongside user_hit_stop.
                        redirect_cause = self.take_session_redirect_cause(thread_id).await;
                        // Drain the device that clicked Cancel (stamped by
                        // `interrupt_agent`) onto `meta` so the terminal
                        // `ResponseCanceled` records it — the popover's Device row.
                        // Cleared again at the Result turn boundary below.
                        if let Some(cancel_actor) = self.take_session_cancel_actor(thread_id).await {
                            meta.actor = Some(cancel_actor);
                        }
                        log!("[AgentSession] Sending control_request interrupt to CC process");
                        if agent_control_tx
                            .send(crate::runtime::ControlRequest::Interrupt)
                            .is_err()
                        {
                            log!("[AgentSession] Failed to forward interrupt: agent control channel closed");
                        }
                        // Arm the bounded fallback: if CC doesn't emit a Result
                        // within the window, the escalation arm hard-stops below.
                        interrupt_escalate_at =
                            Some(tokio::time::Instant::now() + INTERRUPT_ESCALATE_AFTER);
                    }
                    // Don't break — let the loop continue to read the Result event
                }

                _ = stop.notified() => {
                    // Apply, Discard and Archive emit their own lifecycle
                    // terminator.
                    let is_shutdown = self
                        .session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed));
                    let pending_stop = self.pending_stop_reason(thread_id).await;
                    let suppress_user_terminal = matches!(
                        pending_stop,
                        Some(StopReason::Apply | StopReason::Discard | StopReason::Archive),
                    );
                    self.emit_stop_terminal(
                        "stop",
                        thread_id,
                        is_waiting,
                        is_shutdown,
                        suppress_user_terminal,
                        None, // a stop is never a redirect
                        &agent_cancel,
                        &mut claude_text_buf,
                        &meta,
                        &external_terminal_emitted,
                        coding_agent,
                    ).await;
                    emitted_terminal_event = true;
                    self.record_usage_flushed_at_stop(
                        &mut events_rx, thread_id, &external_terminal_emitted, &meta, coding_agent, normalized_model.as_deref(), cc_reasoning_effort.as_deref(),
                    ).await;
                    break;
                }

                // Bounded Esc fallback: CC did not emit a Result within the
                // window after we forwarded the interrupt. Escalate to the hard
                // stop so Cancel is always responsive. Stamp `Canceled(UserStop)`
                // so `finalize` keeps the branch (KeepCanceledBranch) — the
                // session is still best-effort resumable via the `cc_session_id`
                // recorded on `CodingAgentSettingsChanged`. No-worse than the old
                // immediate hard-kill, just delayed for the (rare) ignored-Esc case.
                _ = async move {
                    match escalate_deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    log!(
                        "[AgentSession] CC did not honor interrupt within {}s — escalating to hard stop for thread {}",
                        INTERRUPT_ESCALATE_AFTER.as_secs(),
                        thread_id
                    );
                    last_terminal_kind = Some(TerminalKind::Canceled(
                        redirect_cause.unwrap_or(crate::engine::thread_events::CancelCause::UserStop),
                    ));
                    // In lockstep, because the two describe one terminal. A
                    // cancel is nobody's auto-resume, and carrying an earlier
                    // turn's withheld error here would resume off it.
                    withheld_api_error = None;
                    let is_shutdown = self
                        .session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed));
                    self.emit_stop_terminal(
                        "interrupt_escalate",
                        thread_id,
                        is_waiting,
                        is_shutdown,
                        false, // suppress: a real Cancel — not Apply/Discard/Archive
                        redirect_cause, // a redirect keeps its own cause
                        &agent_cancel,
                        &mut claude_text_buf,
                        &meta,
                        &external_terminal_emitted,
                        coding_agent,
                    ).await;
                    emitted_terminal_event = true;
                    self.record_usage_flushed_at_stop(
                        &mut events_rx, thread_id, &external_terminal_emitted, &meta, coding_agent, normalized_model.as_deref(), cc_reasoning_effort.as_deref(),
                    ).await;
                    break;
                }

                _ = chat_cancel.cancelled() => {
                    // Upstream chat handler cancelled (engine shutdown / request abort).
                    // No user-action context here — suppress flag is always false.
                    let is_shutdown = self
                        .session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed));
                    self.emit_stop_terminal(
                        "chat_cancel",
                        thread_id,
                        is_waiting,
                        is_shutdown,
                        false,
                        None, // chat_cancel is engine shutdown / request abort, not a redirect
                        &agent_cancel,
                        &mut claude_text_buf,
                        &meta,
                        &external_terminal_emitted,
                        coding_agent,
                    ).await;
                    emitted_terminal_event = true;
                    self.record_usage_flushed_at_stop(
                        &mut events_rx, thread_id, &external_terminal_emitted, &meta, coding_agent, normalized_model.as_deref(), cc_reasoning_effort.as_deref(),
                    ).await;
                    break;
                }

                _ = tokio::time::sleep(std::time::Duration::from_secs(WATCHDOG_TICK_INTERVAL_SECS)) => {
                    // Hung-subprocess watchdog. Any incoming event re-arms the
                    // sleep through the surrounding `select!`, so only a fully
                    // silent loop reaches here. On fire, the safety net reads
                    // `watchdog_fired` and emits a `ContinuationRequested`
                    // instead of an abort. The dispatcher then boots a fresh
                    // `--resume` with no user intervention. The diagnostic line
                    // fires on any non-NotStale gate past the threshold, so a
                    // post-mortem can pin which gate held.
                    let (last_ms, session_present) = {
                        let guard = self.agent_sessions.lock().await;
                        match guard.get(&thread_id) {
                            Some(s) => (
                                s.last_event_at.load(std::sync::atomic::Ordering::Relaxed),
                                true,
                            ),
                            None => (0, false),
                        }
                    };
                    let now_ms = now_epoch_millis();
                    let tools_in_flight_snapshot =
                        tools_in_flight.load(std::sync::atomic::Ordering::Relaxed);
                    let gate = watchdog_gate(
                        is_waiting,
                        last_ms,
                        now_ms,
                        WATCHDOG_INACTIVITY_LIMIT_MS,
                        WATCHDOG_HUNG_TOOL_CEILING_MS,
                        tools_in_flight_snapshot,
                    );
                    // last_ms <= 0 routes to SkipBadHeartbeat (no fire) and
                    // would print epoch millis as elapsed — skip the diag.
                    // The Fire branch below is reachable only when last_ms > 0
                    // (the gate's heartbeat check enforces this), so logging
                    // the elapsed there is always meaningful.
                    if last_ms > 0 {
                        let elapsed_ms = now_ms.saturating_sub(last_ms);
                        if elapsed_ms >= WATCHDOG_DIAG_LOG_THRESHOLD_MS
                            && gate != WatchdogGate::NotStale
                        {
                            log!(
                                "[AgentSession] [Watchdog DIAG] thread={} elapsed={}s is_waiting={} tools_in_flight={} session_present={} gate={}",
                                thread_id,
                                elapsed_ms / 1000,
                                is_waiting,
                                tools_in_flight_snapshot,
                                session_present,
                                gate.diag_tag(),
                            );
                        }
                        let should_fire = match gate {
                            WatchdogGate::Fire => true,
                            WatchdogGate::FirePastCeiling(tif) => {
                                // A tool has been in flight past the hung-tool
                                // ceiling. A genuinely hung tool leaves the
                                // thread `running`. A pending question or
                                // permission card flips it to
                                // `waiting_for_user_answer`, yet is deliberately
                                // counted in `tools_in_flight` to disarm the
                                // normal watchdog. Only the first may be killed:
                                // the user may take arbitrarily long to answer.
                                let still_running =
                                    crate::engine::claude_code::thread_is_running(
                                        self.pool(),
                                        thread_id,
                                    )
                                    .await
                                    .unwrap_or(false);
                                if !still_running {
                                    log!(
                                        "[AgentSession] Watchdog: thread {} past hung-tool ceiling with {} tool(s) in flight but no longer `running` (awaiting user answer / already settled) — not firing",
                                        thread_id,
                                        tif
                                    );
                                }
                                still_running
                            }
                            _ => false,
                        };
                        if should_fire {
                            log!(
                                "[AgentSession] Watchdog ({}): no events for {}s while mid-turn for thread {} (hung subprocess / hung tool) — killing the coding-agent subprocess; will auto-resume via ContinuationRequested once events_rx EOFs",
                                gate.diag_tag(),
                                elapsed_ms / 1000,
                                thread_id
                            );
                            agent_cancel.cancel();
                            watchdog_fired = true;
                        }
                    }
                }
            }
        }

        self.finalize_direct_agent(
            thread_id,
            request_id,
            &meta,
            conflict_change.zip(conflict),
            lane_duty,
            lane_answered,
            cwd,
            repo_root,
            branch_name,
            worktree_path,
            images,
            msg_rx,
            claude_text_buf.into_text(),
            last_terminal_kind,
            withheld_api_error,
            external_terminal_emitted,
            external_continuation_requested,
            &agent_cancel,
            emitted_terminal_event,
            watchdog_fired,
            exit_facts,
            init_session_id.is_some(),
            last_emitted_idle,
            is_external_repo,
            proposed_change,
            coding_agent,
        )
        .await
    }
}

/// What the spawn knows about the worktree it is resuming into. Grouped rather
/// than passed as four more parameters: they all come from the same
/// `SpawnWorktreeContext` resolution and are only ever read together.
pub(super) struct ResumeSpawnContext<'a> {
    /// The session's worktree on disk, when it has one.
    pub worktree_path: Option<&'a Path>,
    /// `worktree_head_sha` from the last `CodingAgentIdled`, the baseline the
    /// external-edit detector diffs against. `None` on a first turn.
    pub last_idle_sha: Option<&'a str>,
    /// Pre-built note when the worktree is on a branch holding this agent's
    /// work: an adopted one, or an internal thread's side branch.
    pub branch_note: Option<&'a str>,
    /// The branch this session resumes on, so a discard elsewhere can be told
    /// apart from a discard of the agent's own work.
    pub session_branch: Option<&'a str>,
}

/// Closing line of the resume-note block, between the notes and the user's
/// message.
///
/// The notes quote commit subjects, branch names and file paths. They land in
/// the topic position, right before a message that is often a few words leaning
/// on a pronoun. Those nouns then compete for the pronoun and win. After the
/// Diff-button change applied, a request for more height bound to the note's
/// "composer's Diff". The agent grew the composer rather than the glyph.
pub(super) const RESUME_NOTE_REFERENT_GUARD: &str =
    "[The lines above are engine status, not the topic. The user's message follows. \
     Resolve its pronouns against the conversation you have already had, not against \
     anything named above.]";

/// What a first prompt carries.
pub(super) enum PromptFor<'a> {
    /// A real turn's message, behind its resume notes.
    Turn(&'a str),
    /// A cache keep-alive's nudge, which goes verbatim.
    KeepAlive(&'a str),
}

/// Text for the agent's input channel.
pub(super) struct AgentPrompt {
    pub(super) text: String,
    /// Set when `text` leads with the device block. The block counts as sent
    /// once the agent reads the input, not when it is queued: a withdrawn or
    /// rejected input never reaches the transcript.
    pub(super) device_block: Option<ContextFingerprint>,
}

impl AgentPrompt {
    fn without_block(text: String) -> Self {
        Self {
            text,
            device_block: None,
        }
    }

    /// `text` goes first, then the block when the conversation lacks it.
    fn with_block(text: String, block: Option<DeviceBlock>) -> Self {
        match block {
            Some(block) => Self {
                text: prepend_user_device_preferences_context(&block.text, &text),
                device_block: Some(block.fingerprint),
            },
            None => Self::without_block(text),
        }
    }
}

/// What the conversation a prompt goes to already holds of the device block.
#[derive(Clone, Copy)]
pub(super) struct HeldBlocks<'a> {
    pub(super) sent: &'a SentDeviceContexts,
    /// The backend session id the agent reported at `Init`, or the one a resume
    /// reattaches. A spawn has none, so its first prompt always carries the
    /// block.
    pub(super) conversation: Option<&'a str>,
    /// Blocks queued to the agent and not yet read.
    pub(super) unread: &'a [ContextFingerprint],
}

impl<'a> HeldBlocks<'a> {
    /// Only Claude Code reports a compaction, which can drop the block from
    /// the summary. Another backend holds no conversation, so every message
    /// carries the block.
    pub(super) fn for_agent(
        sent: &'a SentDeviceContexts,
        coding_agent: CodingAgent,
        conversation: Option<&'a str>,
        unread: &'a [ContextFingerprint],
    ) -> Self {
        Self {
            sent,
            conversation: conversation.filter(|_| coding_agent == CodingAgent::ClaudeCode),
            unread,
        }
    }
}

impl HeldBlocks<'_> {
    /// The device block, unless the conversation holds an equal one already.
    async fn block_for_origin(
        self,
        pool: &sqlx::PgPool,
        event_store: &crate::core::EventStore,
        thread_id: Uuid,
        origin_id: Uuid,
    ) -> Option<DeviceBlock> {
        let context =
            crate::engine::agent_context::build_user_device_preferences_context_for_origin(
                pool,
                event_store,
                thread_id,
                origin_id,
            )
            .await;
        self.sent
            .block_for(thread_id, self.conversation, self.unread, &context)
    }
}

/// The first prompt a spawn or resume sends: the resume notes, then the device
/// block unless the resumed conversation holds it already. A slash command goes
/// verbatim ([`forwards_verbatim`]), and so does a keep-alive's nudge.
pub(super) async fn first_prompt_for_agent(
    pool: &sqlx::PgPool,
    event_store: &crate::core::EventStore,
    thread_id: Uuid,
    origin_id: Uuid,
    prompt: PromptFor<'_>,
    spawn: ResumeSpawnContext<'_>,
    held: HeldBlocks<'_>,
) -> AgentPrompt {
    let user_message = match prompt {
        PromptFor::KeepAlive(nudge) => return AgentPrompt::without_block(nudge.to_string()),
        PromptFor::Turn(message) => message,
    };
    if forwards_verbatim(user_message) {
        return AgentPrompt::without_block(user_message.to_string());
    }
    let standing_note = standing_instruction_section(pool, thread_id).await;
    let text = build_resume_prompt_text(
        pool,
        thread_id,
        origin_id,
        user_message,
        standing_note,
        spawn,
    )
    .await;
    let block = held
        .block_for_origin(pool, event_store, thread_id, origin_id)
        .await;
    AgentPrompt::with_block(text, block)
}

/// A follow-up to a live session: the turn-gap note, then the device block of
/// the user who sent it, unless the conversation holds an equal one already. A
/// slash command goes verbatim ([`forwards_verbatim`]).
///
/// The block is judged per follow-up, not at spawn: the session outlives many
/// messages, and each may come from another device or after a preference
/// change. Only the agent sees it, never the `CodingAgentPromptSent` audit
/// event.
pub(super) async fn follow_up_for_agent(
    pool: &sqlx::PgPool,
    event_store: &crate::core::EventStore,
    thread_id: Uuid,
    input: &AgentUserInput,
    prompt_sent: Option<Uuid>,
    session_branch: Option<&str>,
    held: HeldBlocks<'_>,
) -> AgentPrompt {
    if forwards_verbatim(&input.text) {
        return AgentPrompt::without_block(input.text.clone());
    }
    let standing_note = standing_instruction_section(pool, thread_id).await;
    let late_note = match input.kind {
        crate::engine::AgentInputKind::Redelivered { sent_at, input } => Some(
            crate::engine::agent_session::stranded_follow_ups::late_delivery_note(input, sent_at),
        ),
        _ => None,
    };
    let text = build_followup_prompt_text(
        pool,
        thread_id,
        input.origin_event_id,
        prompt_sent,
        &input.text,
        FollowUpNotes {
            late: late_note.as_deref(),
            standing: standing_note,
        },
        session_branch,
    )
    .await;
    let Some(origin) = input.user_origin() else {
        return AgentPrompt::without_block(text);
    };
    let block = held
        .block_for_origin(pool, event_store, thread_id, origin)
        .await;
    AgentPrompt::with_block(text, block)
}

/// Build the text handed to the resumed agent's input channel: the user's
/// message, optionally prefixed with up to three resume-time notes that
/// reconcile what changed while the agent was idle:
///
/// 1. **branch adoption**: the worktree switched to a new branch holding the
///    agent's work,
/// 2. **turn gap**: the user or the engine resolved one of the agent's changes,
///    or the cleanup worker reclaimed the worktree,
/// 3. **external edits**: the worktree changed under the agent.
///
/// The `standing_note` ([`standing_instruction_section`]) leads them, since it
/// is about this turn rather than the gap before it.
///
/// The turn-gap note is computed FIRST because it decides whether the
/// external-edit note may report a HEAD move as unexplained. An Apply, a
/// Discard or a tier-2 worktree clean moves HEAD without anyone editing a file,
/// and the edit detector cannot tell the difference. The turn-gap note states
/// the real cause, so it passes `explains_worktree_reset` down and the edit
/// note drops that one line.
///
/// A non-empty block closes with [`RESUME_NOTE_REFERENT_GUARD`], so the notes
/// cannot be read as the subject of the message they precede.
///
/// An empty `user_message` passes through untouched: notes only ride on a real
/// turn, so they cannot trigger an otherwise-empty LLM call.
pub(super) async fn build_resume_prompt_text(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    current_origin_id: Uuid,
    user_message: &str,
    standing_note: &str,
    spawn: ResumeSpawnContext<'_>,
) -> String {
    if user_message.is_empty() {
        return user_message.to_string();
    }

    let ResumeSpawnContext {
        worktree_path,
        last_idle_sha,
        branch_note,
        session_branch,
    } = spawn;

    // What the user and the engine did to this agent's work in the gap before
    // this turn. `current_origin_id` bounds the lookup to the previous turn
    // boundary: stateless and self-clearing, see `compute_turn_gap_note`.
    let gap_note = crate::engine::agent_session::turn_gap::compute_turn_gap_note(
        pool,
        thread_id,
        current_origin_id,
        session_branch,
    )
    .await;

    let edit_note = match (worktree_path, last_idle_sha) {
        (Some(wt), Some(sha)) => {
            crate::engine::agent_session::external_edits::compute_external_edit_note(
                wt,
                Some(sha),
                gap_note.as_ref().is_some_and(|n| n.explains_worktree_reset),
            )
            .await
        }
        _ => None,
    };

    // Fold the resume-time notes into one prepended block, cause before
    // observation: the gap note explains a reset the edit note can only see the
    // effects of.
    let combined: Vec<&str> = [
        turn_note(standing_note),
        branch_note,
        gap_note.as_ref().map(|n| n.note.as_str()),
        edit_note.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    prepend_resume_notes(thread_id, &combined, user_message)
}

/// A per-turn section as a note, or `None` when it is empty.
fn turn_note(section: &str) -> Option<&str> {
    Some(section.trim()).filter(|note| !note.is_empty())
}

/// The notes a follow-up carries besides the turn-gap note.
#[derive(Clone, Copy, Default)]
pub(super) struct FollowUpNotes<'a> {
    /// The message is a redelivery a dead process never read.
    pub late: Option<&'a str>,
    /// [`standing_instruction_section`].
    pub standing: &'a str,
}

/// The text a follow-up to a live session carries: the message, behind its
/// `notes` and the turn-gap note when there are any. A kept-alive session
/// spawns nothing, so without this an event from its previous turn reaches no
/// prompt.
///
/// `prompt_sent` is the `CodingAgentPromptSent` this forward wrote before it
/// computed the note. Only a user-kind input writes one. See
/// [`followup_gap_bounds`].
pub(super) async fn build_followup_prompt_text(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    origin: Option<Uuid>,
    prompt_sent: Option<Uuid>,
    message: &str,
    notes: FollowUpNotes<'_>,
    session_branch: Option<&str>,
) -> String {
    // Unlike a spawn, no empty-message skip: the forward runs either way, and
    // an image-only follow-up still writes the boundary this note ends at.
    let gap_note = match followup_gap_bounds(origin, prompt_sent) {
        Some((anchor, until)) => {
            crate::engine::agent_session::turn_gap::compute_turn_gap_note_until(
                pool,
                thread_id,
                anchor,
                until,
                session_branch,
            )
            .await
        }
        None => None,
    };
    let notes: Vec<&str> = [
        notes.late,
        turn_note(notes.standing),
        gap_note.as_ref().map(|n| n.note.as_str()),
    ]
    .into_iter()
    .flatten()
    .collect();
    prepend_resume_notes(thread_id, &notes, message)
}

/// The events a forwarded input owes its read to. A child wake owes its
/// `ChildThreadCompleted`, so its read can say it started a turn. An engine
/// prompt with no starter event (auto-harden) owes its `CodingAgentPromptSent`,
/// so its read confirms that turn-gap boundary.
pub(super) fn owed_input_event_ids(origin: Option<Uuid>, prompt_sent: Option<Uuid>) -> Vec<Uuid> {
    origin.or(prompt_sent).into_iter().collect()
}

/// The `(anchor, until)` a live session's turn-gap note takes.
///
/// A forward that wrote a `CodingAgentPromptSent` ends at it, since that
/// boundary is the next note's lower bound. One that wrote none ends at its
/// origin. With neither there is no note, and no boundary hides the gap from
/// the next prompt.
pub(super) fn followup_gap_bounds(
    origin: Option<Uuid>,
    prompt_sent: Option<Uuid>,
) -> Option<(Uuid, Uuid)> {
    match (origin, prompt_sent) {
        (anchor, Some(sent)) => Some((anchor.unwrap_or(sent), sent)),
        (Some(origin), None) => Some((origin, origin)),
        (None, None) => None,
    }
}

/// `message` behind the resume-time `notes`, closed by
/// [`RESUME_NOTE_REFERENT_GUARD`]. No notes leaves it untouched.
fn prepend_resume_notes(thread_id: Uuid, notes: &[&str], message: &str) -> String {
    if notes.is_empty() {
        return message.to_string();
    }
    let block = notes.join("\n");
    log!(
        "[AgentSession] Injecting resume note for thread {} ({} chars)",
        thread_id,
        block.len()
    );
    format!("{block}\n{RESUME_NOTE_REFERENT_GUARD}\n\n{message}")
}

/// Longest a message may run inside the thread-history block.
const HISTORY_MESSAGE_MAX_BYTES: usize = 2000;

/// Budget for the whole thread-history block. History only grows, and an
/// uncapped block would grow every fresh session's first message with it.
const HISTORY_BLOCK_MAX_BYTES: usize = 48 * 1024;

/// Most images of unanswered messages a fresh session's first message
/// carries. The newest are kept, so a run of stopped turns cannot push the
/// request past a provider's image or size limit.
const UNANSWERED_IMAGES_MAX: usize = 8;

/// One thread message as the thread-history block reads it.
#[derive(Debug, Clone, PartialEq)]
struct HistoryLine<'a> {
    role: &'a str,
    channel: Option<&'a str>,
    content: &'a str,
    image_hashes: &'a [String],
    event_id: Option<&'a str>,
}

impl<'a> HistoryLine<'a> {
    fn of(message: &'a crate::core::store::SessionMessage) -> Self {
        Self {
            role: &message.role,
            channel: message.channel.as_deref(),
            content: &message.content,
            image_hashes: &message.user_image_hashes,
            event_id: message.event_id.as_deref(),
        }
    }

    /// A coding agent wrote it. An `[ERROR]` line or an `[Asked for you]`
    /// note carries no channel: no agent read the message before it.
    fn is_reply(&self) -> bool {
        self.role == "assistant"
            && self.channel == Some("claude_code")
            && !self.content.trim().is_empty()
    }
}

/// `messages` without the one being sent, and where it stood: the last user
/// message reading `current`, else the end. That message follows the session
/// context, so the history must not repeat it.
fn before_current_message<'a>(
    mut messages: Vec<HistoryLine<'a>>,
    current: &str,
) -> (Vec<HistoryLine<'a>>, usize) {
    match messages
        .iter()
        .rposition(|line| line.role == "user" && line.content.trim() == current.trim())
    {
        Some(at) => {
            messages.remove(at);
            (messages, at)
        }
        None => {
            let end = messages.len();
            (messages, end)
        }
    }
}

/// Where the messages no agent answered begin: just after the last reply.
/// A message stopped before any session started has no reply, so a fresh
/// session is the first agent to read it.
fn first_unanswered(messages: &[HistoryLine<'_>]) -> usize {
    messages
        .iter()
        .rposition(HistoryLine::is_reply)
        .map_or(0, |at| at + 1)
}

/// The newest [`UNANSWERED_IMAGES_MAX`] images on the messages no agent
/// answered, oldest first. `redelivered` names messages that reach the
/// session again with their own images, so they bring none here.
fn unanswered_image_hashes<'a>(
    messages: &[HistoryLine<'a>],
    redelivered: &[String],
) -> Vec<&'a str> {
    let mut hashes: Vec<&str> = messages[first_unanswered(messages)..]
        .iter()
        .filter(|line| line.role == "user")
        .filter(|line| {
            line.event_id
                .is_none_or(|id| !redelivered.iter().any(|r| r == id))
        })
        .flat_map(|line| line.image_hashes.iter().map(String::as_str))
        .collect();
    hashes.drain(..hashes.len().saturating_sub(UNANSWERED_IMAGES_MAX));
    hashes
}

/// The thread history a new coding-agent session sees. Each message's
/// images are counted as `attached` to the first message or not shown. The
/// newest messages are kept when the whole thread exceeds
/// [`HISTORY_BLOCK_MAX_BYTES`]. Empty when no message has content.
fn thread_history_block(messages: &[HistoryLine<'_>], attached: &[&str]) -> String {
    let entries: Vec<String> = messages
        .iter()
        .filter_map(|line| {
            let content = line.content.trim();
            let images = if line.role == "user" {
                line.image_hashes
            } else {
                &[]
            };
            if content.is_empty() && images.is_empty() {
                return None;
            }
            let label = match line.role {
                "user" => "User",
                "assistant" if line.channel == Some("claude_code") => "Coding agent",
                "assistant" => "Assistant",
                other => other,
            };
            let mut body = if content.len() > HISTORY_MESSAGE_MAX_BYTES {
                let end = content.floor_char_boundary(HISTORY_MESSAGE_MAX_BYTES);
                format!(
                    "{}…\n[truncated, {} chars total]",
                    &content[..end],
                    content.len()
                )
            } else {
                content.to_string()
            };
            let shown = images
                .iter()
                .filter(|hash| attached.contains(&hash.as_str()))
                .count();
            if shown > 0 {
                body.push_str(&format!(
                    "\n[{shown} image(s), attached to the message after this context]"
                ));
            }
            if images.len() > shown {
                body.push_str(&format!(
                    "\n[{} image(s), not shown to this session]",
                    images.len() - shown
                ));
            }
            Some(format!("**{}:** {}\n\n", label, body.trim_start()))
        })
        .collect();
    if entries.is_empty() {
        return String::new();
    }
    let mut kept_bytes = 0;
    let kept = entries
        .iter()
        .rev()
        .take_while(|entry| {
            kept_bytes += entry.len();
            kept_bytes <= HISTORY_BLOCK_MAX_BYTES
        })
        .count();
    let omitted = entries.len() - kept;
    let mut history = String::from(
        "THREAD HISTORY: This session continues an existing thread. Here is the conversation \
         before this message:\n\n",
    );
    if omitted > 0 {
        history.push_str(&format!("[{omitted} earlier messages omitted]\n\n"));
    }
    for entry in &entries[omitted..] {
        history.push_str(entry);
    }
    history
}

#[cfg(test)]
mod thread_history_block_tests {
    use super::*;

    fn line<'a>(role: &'a str, channel: Option<&'a str>, content: &'a str) -> HistoryLine<'a> {
        HistoryLine {
            role,
            channel,
            content,
            image_hashes: &[],
            event_id: None,
        }
    }

    fn with_images<'a>(line: HistoryLine<'a>, image_hashes: &'a [String]) -> HistoryLine<'a> {
        HistoryLine {
            image_hashes,
            ..line
        }
    }

    /// A long thread's history stays under budget and keeps the newest
    /// messages.
    #[test]
    fn a_long_thread_keeps_the_newest_messages_within_budget() {
        let contents: Vec<String> = (0..400)
            .map(|i| format!("message {i:03} {}", "x".repeat(1500)))
            .collect();
        let lines: Vec<_> = contents.iter().map(|c| line("user", None, c)).collect();
        let block = thread_history_block(&lines, &[]);
        assert!(
            block.len() < HISTORY_BLOCK_MAX_BYTES + 1024,
            "{} bytes",
            block.len()
        );
        assert!(
            block.contains("message 399"),
            "the newest message was dropped"
        );
        assert!(
            !block.contains("message 000"),
            "the oldest message survived"
        );
        assert!(block.contains("earlier messages omitted"));
    }

    #[test]
    fn a_short_thread_is_kept_whole() {
        let block = thread_history_block(
            &[
                line("user", None, "hello"),
                line("assistant", Some("claude_code"), "hi there"),
            ],
            &[],
        );
        assert!(block.contains("**User:** hello"));
        assert!(block.contains("**Coding agent:** hi there"));
        assert!(!block.contains("omitted"));
        assert_eq!(thread_history_block(&[line("user", None, "  ")], &[]), "");
    }

    /// The message being sent follows the session context, so the history
    /// holds only what came before it.
    #[test]
    fn the_history_leaves_out_the_message_being_sent() {
        let lines = vec![
            line("user", None, "fix the drawer"),
            line("assistant", Some("claude_code"), "fixed"),
            line("user", None, "fix the drawer"),
        ];
        let (prior, at) = before_current_message(lines, " fix the drawer\n");
        assert_eq!(
            prior,
            vec![
                line("user", None, "fix the drawer"),
                line("assistant", Some("claude_code"), "fixed"),
            ],
            "only the newest copy is the message being sent"
        );
        assert_eq!(at, 2);
        let block = thread_history_block(&prior, &[]);
        assert_eq!(block.matches("fix the drawer").count(), 1, "{block}");
    }

    /// A turn no user message opened (a child's report, an engine prompt)
    /// leaves the history whole.
    #[test]
    fn a_turn_without_a_matching_message_keeps_the_whole_history() {
        let lines = vec![line("user", None, "hello"), line("assistant", None, "hi")];
        assert_eq!(
            before_current_message(lines.clone(), "Your child thread finished."),
            (lines, 2)
        );
    }

    /// A fresh session gets the images of messages no agent answered, not
    /// only their text. An error line is no answer.
    #[test]
    fn a_fresh_session_gets_the_images_no_agent_answered() {
        let answered = ["old".to_string()];
        let stopped = ["shot".to_string()];
        let lines = vec![
            with_images(line("user", None, "look at this"), &answered),
            line("assistant", Some("claude_code"), "done"),
            with_images(line("user", None, "drop the descriptions here"), &stopped),
            line("assistant", None, "[ERROR] **Error:** startup failed"),
            line("user", None, "continue"),
        ];
        let (prior, at) = before_current_message(lines, "continue");
        let hashes = unanswered_image_hashes(&prior[..at], &[]);
        assert_eq!(hashes, ["shot"]);
        let block = thread_history_block(&prior, &hashes);
        assert!(
            block.contains("look at this\n[1 image(s), not shown to this session]"),
            "{block}"
        );
        assert!(
            block.contains(
                "drop the descriptions here\n[1 image(s), attached to the message after this context]"
            ),
            "{block}"
        );
    }

    /// A stranded follow-up and a message after the current one each reach
    /// the session with their own images. Neither brings them into the block.
    #[test]
    fn redelivered_and_later_messages_bring_no_images() {
        let stranded = ["stranded".to_string()];
        let queued = ["queued".to_string()];
        let lines = vec![
            HistoryLine {
                event_id: Some("evt-stranded"),
                ..with_images(line("user", None, "and this"), &stranded)
            },
            line("user", None, "go on"),
            with_images(line("user", None, "one more"), &queued),
        ];
        let (prior, at) = before_current_message(lines, "go on");
        assert!(unanswered_image_hashes(&prior[..at], &["evt-stranded".to_string()]).is_empty());
    }

    /// Only the newest images ride along, so a run of stopped turns stays
    /// within a provider's limits.
    #[test]
    fn the_newest_unanswered_images_are_kept() {
        let hashes: Vec<String> = (0..UNANSWERED_IMAGES_MAX + 3)
            .map(|i| format!("img{i:02}"))
            .collect();
        let lines = vec![with_images(line("user", None, "many"), &hashes)];
        let kept = unanswered_image_hashes(&lines, &[]);
        assert_eq!(kept.len(), UNANSWERED_IMAGES_MAX);
        assert_eq!(kept.first(), Some(&"img03"));
        assert_eq!(kept.last(), Some(&hashes.last().unwrap().as_str()));
    }

    /// An image that could not be read is not called attached.
    #[test]
    fn an_image_that_failed_to_load_reads_as_not_shown() {
        let hashes = ["here".to_string(), "gone".to_string()];
        let block = thread_history_block(
            &[with_images(line("user", None, "two"), &hashes)],
            &["here"],
        );
        assert!(
            block.contains(
                "two\n[1 image(s), attached to the message after this context]\n[1 image(s), not shown to this session]"
            ),
            "{block}"
        );
    }

    /// A message of only images still shows in the history.
    #[test]
    fn an_image_only_message_is_kept() {
        let hashes = ["shot".to_string()];
        let block =
            thread_history_block(&[with_images(line("user", None, ""), &hashes)], &["shot"]);
        assert!(
            block.contains("**User:** [1 image(s), attached to the message after this context]"),
            "{block}"
        );
    }
}

#[cfg(test)]
mod unregistered_lucidos_root_tests {
    use super::*;

    /// The regression. On a packaged install `main_worktree()` resolves to the
    /// WORKSPACE dir; handing that back would branch the user's workspace git
    /// and call it Lucidos platform source. Refuse instead.
    #[test]
    fn refuses_when_there_is_no_source_checkout() {
        let workspace_dir = PathBuf::from("/Users/me/Library/Application Support/lucidos/ws");
        let err = unregistered_lucidos_root(workspace_dir, false)
            .expect_err("a Lucidos-source spawn with no source checkout must be refused")
            .to_string();
        assert!(
            err.contains("no source checkout"),
            "refusal must say why: {err}"
        );
        assert!(
            err.contains("data/apps/<id>"),
            "refusal must name the routes that still work: {err}"
        );
    }

    /// The fallback's legitimate case survives untouched: a dev build whose
    /// `Lucidos` registry row hasn't been written yet (very early startup).
    #[test]
    fn keeps_the_early_startup_fallback_on_a_dev_build() {
        let dev_root = PathBuf::from("/Users/me/src/lucidos");
        assert_eq!(
            unregistered_lucidos_root(dev_root.clone(), true)
                .expect("dev build must still resolve"),
            dev_root
        );
    }
}

#[cfg(test)]
mod startup_failure_tests {
    use super::*;

    const GATEWAY_MESSAGE: &str = "Couldn't load settings from Cloud gateway \
https://claude-gateway.example.com. Check your network connection, or sign in again with /login.";

    fn exited(exit: AgentExitFacts) -> AgentEvent {
        AgentEvent::Exited { exit }
    }

    fn drained(events: Vec<AgentEvent>) -> StartupDeath {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        for ev in events {
            tx.send(ev).unwrap();
        }
        drop(tx); // driver wound down
        drain_startup_death(&mut rx)
    }

    #[test]
    fn recovers_driver_result_error() {
        let death = drained(vec![
            AgentEvent::Result {
                text: String::new(),
                duration_ms: 0,
                error: Some("Failed to start Codex: No such file or directory".to_string()),
            },
            exited(AgentExitFacts::default()),
        ]);
        assert_eq!(
            death.reason.as_deref(),
            Some("Failed to start Codex: No such file or directory")
        );
    }

    #[test]
    fn the_cli_words_on_stderr_are_the_reason_when_the_driver_sent_no_error() {
        let death = drained(vec![exited(AgentExitFacts {
            killed_by_signal: false,
            code: Some(1),
            stderr_tail: GATEWAY_MESSAGE.to_string(),
        })]);
        assert_eq!(death.reason.as_deref(), Some(GATEWAY_MESSAGE));
        assert_eq!(death.exit.code, Some(1));
    }

    #[test]
    fn falls_back_to_signal_kill_when_no_result_error() {
        let death = drained(vec![exited(AgentExitFacts {
            killed_by_signal: true,
            ..AgentExitFacts::default()
        })]);
        assert_eq!(
            death.reason.as_deref(),
            Some("coding agent process was killed by a signal during startup")
        );
    }

    #[test]
    fn none_when_no_failure_signal() {
        // Clean exit, no error, no signal — caller uses its generic message.
        let death = drained(vec![exited(AgentExitFacts::default())]);
        assert_eq!(death.reason, None);
    }

    #[test]
    fn ignores_empty_error_strings() {
        let death = drained(vec![AgentEvent::Result {
            text: String::new(),
            duration_ms: 0,
            error: Some("   ".to_string()),
        }]);
        assert_eq!(death.reason, None);
    }

    #[test]
    fn a_death_with_the_cli_words_becomes_a_typed_startup_failure() {
        let death = drained(vec![exited(AgentExitFacts {
            killed_by_signal: false,
            code: Some(1),
            stderr_tail: GATEWAY_MESSAGE.to_string(),
        })]);
        let error = startup_failure_error(CodingAgent::ClaudeCode, death, "plain".into());
        assert_eq!(error.to_string(), GATEWAY_MESSAGE);
        assert_eq!(
            crate::engine::agent_session::start_failure::agent_exit_of(error.as_ref()),
            Some(CodingAgentExit {
                stage: CodingAgentExitStage::Startup,
                code: Some(1)
            })
        );
    }

    #[test]
    fn a_silent_death_with_an_exit_code_names_the_agent() {
        let death = drained(vec![exited(AgentExitFacts {
            code: Some(2),
            ..AgentExitFacts::default()
        })]);
        let error = startup_failure_error(CodingAgent::Codex, death, "plain".into());
        assert_eq!(
            error.to_string(),
            "Codex exited with code 2 before its session began and wrote nothing to stderr."
        );
    }

    #[test]
    fn a_death_that_left_no_facts_keeps_the_plain_message() {
        let error = startup_failure_error(
            CodingAgent::ClaudeCode,
            StartupDeath::default(),
            "plain".into(),
        );
        assert_eq!(error.to_string(), "plain");
        assert_eq!(
            crate::engine::agent_session::start_failure::agent_exit_of(error.as_ref()),
            None
        );
    }

    const NOT_FOUND: &str = "No conversation found with session ID: 2f27ec90";

    #[test]
    fn a_refused_resume_is_a_stale_resume() {
        assert_eq!(
            classify_startup_failure(true, Some(NOT_FOUND.to_string())),
            StartupFailure::StaleResume
        );
    }

    #[test]
    fn the_same_text_on_a_fresh_spawn_is_a_plain_failure() {
        assert!(matches!(
            classify_startup_failure(false, Some(NOT_FOUND.to_string())),
            StartupFailure::Failed(e) if e.ends_with(NOT_FOUND)
        ));
    }

    #[test]
    fn any_other_resume_failure_stays_a_plain_failure() {
        assert!(matches!(
            classify_startup_failure(true, Some("API Error: 529 overloaded".to_string())),
            StartupFailure::Failed(_)
        ));
        assert!(matches!(
            classify_startup_failure(true, None),
            StartupFailure::Failed(_)
        ));
    }
}

/// A turn that streamed no assistant text states its failure ONCE: in the
/// failure card, never also as a paragraph of the response.
#[cfg(test)]
mod result_text_prose_tests {
    use super::result_text_is_own_prose;

    /// The case the branch exists for: `/model` answers through `result.result`
    /// with no preceding Message, and no failure to collide with.
    #[test]
    fn a_slash_command_answer_is_prose() {
        assert!(result_text_is_own_prose("Set model to claude-opus-5", None));
    }

    /// The duplicate. CC's own banner is no longer buffered as text. A turn
    /// whose only output was that banner reaches this branch with the failure
    /// reason in `result.result`. Emitting it puts the card's sentence back in
    /// the response body.
    #[test]
    fn the_turns_failure_reason_is_not_prose() {
        let err = "API Error: Stream idle timeout - no chunks received";
        assert!(!result_text_is_own_prose(err, Some(err)));
        assert!(
            !result_text_is_own_prose(&format!("\n\n{err}\n"), Some(err)),
            "compared trimmed: the buffered copy carries the stream's leading newlines"
        );
    }

    /// Equality, not an `API Error` prefix test. A failure whose reason came
    /// from elsewhere leaves the text alone rather than guessing, and the
    /// frontend's echo drop is the backstop.
    #[test]
    fn text_the_card_will_not_show_verbatim_stays() {
        assert!(result_text_is_own_prose(
            "API Error: 500 upstream said no",
            Some("error_during_execution")
        ));
    }

    #[test]
    fn empty_text_is_never_emitted() {
        assert!(!result_text_is_own_prose("", None));
        assert!(!result_text_is_own_prose("   \n ", None));
    }
}

/// A hard stop leaves the event loop at once, so the usage Claude Code's
/// driver flushes for a cut-off call is read by `usage_flushed_at_stop`.
#[cfg(test)]
mod usage_flushed_at_stop_tests {
    use super::{usage_flushed_at_stop, AgentEvent};
    use tokio::sync::mpsc;

    fn usage(cache_read_tokens: u32) -> AgentEvent {
        AgentEvent::Usage {
            model: Some("claude-sonnet-5".into()),
            input_tokens: 2,
            output_tokens: 4,
            cache_read_tokens,
            cache_creation_tokens: 0,
            cache_creation_1h_tokens: None,
            parent_tool_use_id: None,
            api_call_id: None,
            duration_ms: None,
            reasoning_effort: None,
        }
    }

    /// Every `Usage` up to `OutputEnded` is kept, and nothing after it is read.
    #[tokio::test]
    async fn it_keeps_each_usage_up_to_output_ended() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        for ev in [
            usage(100),
            AgentEvent::StreamActivity,
            usage(200),
            AgentEvent::OutputEnded,
            usage(300),
        ] {
            tx.send(ev).expect("send");
        }

        let flushed = usage_flushed_at_stop(&mut rx, std::time::Duration::from_secs(5)).await;

        let cache_reads: Vec<u32> = flushed.iter().map(|(_, u)| u.cache_read_tokens).collect();
        assert_eq!(cache_reads, vec![100, 200]);
        assert_eq!(flushed[0].0.as_deref(), Some("claude-sonnet-5"));
        assert!(
            matches!(rx.try_recv(), Ok(AgentEvent::Usage { .. })),
            "the read stops at OutputEnded"
        );
    }

    /// A sub-agent's flushed call keeps its parent, so its capture still binds
    /// to that sub-agent's steps.
    #[tokio::test]
    async fn a_flushed_sub_agent_call_keeps_its_parent() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(AgentEvent::Usage {
            model: None,
            input_tokens: 2,
            output_tokens: 4,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cache_creation_1h_tokens: None,
            parent_tool_use_id: Some("toolu_agent".into()),
            api_call_id: None,
            duration_ms: None,
            reasoning_effort: None,
        })
        .expect("send");
        drop(tx);

        let flushed = usage_flushed_at_stop(&mut rx, std::time::Duration::from_secs(5)).await;

        assert_eq!(
            flushed[0].1.parent_tool_use_id.as_deref(),
            Some("toolu_agent")
        );
    }

    /// A closed channel ends the read too, with whatever arrived first.
    #[tokio::test]
    async fn a_closed_channel_ends_the_read() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(usage(7)).expect("send");
        drop(tx);

        let flushed = usage_flushed_at_stop(&mut rx, std::time::Duration::from_secs(5)).await;

        assert_eq!(flushed.len(), 1);
    }

    /// A driver that never says it is done cannot hold the stop past `wait`.
    #[tokio::test(start_paused = true)]
    async fn a_silent_driver_cannot_hold_the_stop() {
        let (_tx, mut rx) = mpsc::unbounded_channel();

        let flushed = usage_flushed_at_stop(&mut rx, std::time::Duration::from_secs(2)).await;

        assert!(flushed.is_empty());
    }
}

/// Each coding-agent call records the model the reply named, never a picker
/// alias, so Token Cost can price it.
#[cfg(test)]
mod call_models_tests {
    use super::CallModels;
    use crate::llm::served_model::served_as_sent;

    /// The unpriced calls: a sub-agent set to `model: haiku` on an Opus
    /// session recorded `haiku`. It records the id that answered, and reads
    /// as served as sent, so no reader takes it for a reroute.
    #[test]
    fn an_alias_sub_agent_call_records_the_model_that_answered() {
        let models = CallModels::of(
            Some("claude-opus-5-5[1m]"),
            Some("claude-haiku-4-5-20251001"),
        );

        assert_eq!(
            models,
            CallModels {
                model: "claude-haiku-4-5-20251001".into(),
                served_model: Some("claude-haiku-4-5-20251001".into()),
            }
        );
    }

    /// A session started on an alias with `[1m]` keeps the marker on the full
    /// id, so the long context window still reaches a reader.
    #[test]
    fn an_alias_session_with_1m_keeps_the_marker_on_the_full_id() {
        let models = CallModels::of(Some("opus[1m]"), Some("claude-opus-4-6"));

        assert_eq!(models.model, "claude-opus-4-6[1m]");
        assert!(served_as_sent(&models.model, "claude-opus-4-6"));
    }

    /// A sub-agent on another full id keeps that id in both fields, not the
    /// session's.
    #[test]
    fn a_full_id_call_records_its_own_id_twice() {
        let models = CallModels::of(Some("claude-opus-5-5[1m]"), Some("claude-sonnet-5"));

        assert_eq!(
            models,
            CallModels {
                model: "claude-sonnet-5".into(),
                served_model: Some("claude-sonnet-5".into()),
            }
        );
    }

    /// The session's own call keeps its `[1m]` in `model`, and reads as
    /// served as sent.
    #[test]
    fn a_main_session_call_is_served_as_sent() {
        let models = CallModels::of(Some("claude-opus-5-5[1m]"), Some("claude-opus-5-5"));

        assert_eq!(models.model, "claude-opus-5-5[1m]");
        assert_eq!(models.served_model.as_deref(), Some("claude-opus-5-5"));
        assert!(served_as_sent(&models.model, "claude-opus-5-5"));
    }

    /// Codex echoes no model per call. The session's model stands in for
    /// `model`, and no reply named a served one.
    #[test]
    fn a_call_with_no_echoed_model_records_no_served_model() {
        for reported in [None, Some("")] {
            assert_eq!(
                CallModels::of(Some("gpt-5.6-sol"), reported),
                CallModels {
                    model: "gpt-5.6-sol".into(),
                    served_model: None,
                }
            );
        }
    }
}

/// A coding-agent `ContextCaptured` names the effort the call ran at, under
/// the field the main-LLM rows use, so one rollup groups every producer.
#[cfg(test)]
mod coding_agent_capture_tests {
    use super::{coding_agent_capture, effort_in_force, CallModels, ReportedUsage};
    use crate::runtime::CodingAgent;

    fn reported() -> ReportedUsage {
        ReportedUsage {
            input_tokens: 2,
            output_tokens: 4,
            cache_read_tokens: 8,
            cache_creation_tokens: 0,
            cache_creation_1h_tokens: None,
            parent_tool_use_id: None,
            api_call_id: None,
            duration_ms: None,
            reasoning_effort: None,
        }
    }

    fn captured_payload(effort: Option<String>) -> serde_json::Value {
        let event = coding_agent_capture(
            CodingAgent::ClaudeCode,
            CallModels::of(Some("claude-opus-5-5"), Some("claude-opus-5-5")),
            200_000,
            reported(),
            effort,
        );
        serde_json::to_value(&event).expect("serializes")
    }

    #[test]
    fn a_claude_code_capture_carries_the_effort_the_session_ran_at() {
        let payload = captured_payload(Some("high".into()));

        assert_eq!(payload["producer"], "claude_code");
        assert_eq!(payload["reasoning_effort"], "high");
    }

    /// An unknown effort is left out, never guessed.
    #[test]
    fn an_unknown_effort_is_omitted() {
        let payload = captured_payload(None);

        assert!(payload.get("reasoning_effort").is_none(), "{payload}");
    }

    /// Claude Code takes effort only at spawn, so every call runs at the
    /// spawn effort.
    #[test]
    fn claude_code_runs_at_its_spawn_effort() {
        assert_eq!(
            effort_in_force(CodingAgent::ClaudeCode, Some("medium"), Some("max".into())),
            Some("medium".into())
        );
    }

    /// Codex applies an effort change at its next turn, so the effort its
    /// driver sent with the call's turn is the one in force. A turn sent
    /// with none ran at an effort the driver cannot name.
    #[test]
    fn codex_runs_at_the_effort_its_driver_sent() {
        assert_eq!(
            effort_in_force(CodingAgent::Codex, Some("medium"), Some("high".into())),
            Some("high".into())
        );
        assert_eq!(
            effort_in_force(CodingAgent::Codex, Some("medium"), None),
            None
        );
    }
}
