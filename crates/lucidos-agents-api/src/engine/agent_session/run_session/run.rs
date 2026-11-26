use super::completion::{idle_exit, IdleExit};
use super::idle_change_state::{resolve_idle_change_state, IdleChangeStateInput};
use super::idle_snapshot::{CodingAgentIdleSnapshot, IdleTurn};
use super::spawn_context::SpawnWorktreeContext;
use crate::engine::agent_context::{
    prepend_user_device_preferences_context, ContextFingerprint, DeviceBlock, SentDeviceContexts,
};
use crate::engine::agent_session::run_session::completion::CompletionExt;
use crate::engine::agent_session::run_session::idle_snapshot::IdleSnapshotExt;
use crate::engine::agent_session::run_session::spawn_context::SpawnContextExt;
use crate::engine::agent_session::runtime_helpers::RuntimeHelpersExt;
use crate::engine::agent_session::side_question::AgentSessionSideQuestionExt;
use crate::engine::agent_session::spawn_claim::SpawnClaimExt;
use crate::engine::agent_session::spawn_gate::SpawnGateExt;
use crate::engine::agent_session::stranded_follow_ups::StrandedFollowUpsExt;
use crate::engine::agent_session::text_buffer::CodingAgentTextBuffer;
use crate::engine::agentic_loop::should_flush;
use crate::engine::change_ops::{now_epoch_millis, ProposeOutcome};
use crate::engine::claude_code::control::ControlExt;
use crate::engine::claude_code::{CcModelDiscoveryExt, STALE_RESUME_ERROR};
use crate::engine::event_wait::OwnerTurn;
use crate::engine::git_ops::{
    auto_commit_preserving_marker, branch_head_sha, default_local_branch, describe_branch_changes,
    files_require_restart, has_full_harden_marker, is_external_repo_path, main_worktree,
};
use crate::engine::start_failure::AgentStartFailure;
use crate::engine::thread_events::{
    CodingAgentExit, CodingAgentExitStage, EventChannel, SessionEndReason,
};
use crate::engine::turn_end::read_decision::TurnEndDecisionExt;
use crate::engine::{AgentUserInput, LucidosEngine, ProcessResult, QuietPick, StopReason};
use crate::runtime::agent_runtime::forwards_verbatim;
use crate::runtime::billing_provider::BillingAttribution;
use crate::runtime::{AgentEvent, AgentExitFacts, AgentInput, CodingAgent};
use futures::StreamExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::engine::agent_session::child_reports::{begin_take, settle_take};
use crate::engine::agent_session::continuity::resolve_session_continuity;
use crate::engine::agent_session::input_ledger::{announce_reads, InputLedger, Settled};
use crate::engine::agent_session::io_helpers::requeue_at_head;
use crate::engine::agent_session::lifecycle::{
    ceiling_turn_end, classify_result, idle_action, idle_change_write,
    is_definitive_session_not_found, is_resume_settle_result, is_stale_resume_signal,
    model_swap_reentry_due, require_agent_input, reset_per_turn_flags, result_auto_commits,
    should_auto_commit_on_cleanup, starts_turn_after_terminal, terminal_clears_user_hit_stop,
    terminate_decision, turn_ended_cleanly, turn_has_terminal, watchdog_gate, CeilingTurnEnd,
    IdleAction, IdleChangeWrite, IdleSession, ModelSwapIdle, StaleResumeInputs, TerminalKind,
    TerminateDecision, TurnTerminal, WatchdogGate, WATCHDOG_DIAG_LOG_THRESHOLD_MS,
    WATCHDOG_HUNG_TOOL_CEILING_MS, WATCHDOG_INACTIVITY_LIMIT_MS, WATCHDOG_TICK_INTERVAL_SECS,
};
use crate::engine::agent_session::prompts::standing_instruction_section;
use crate::engine::agent_session::resume::{resolve_run_resume, RunKind, RunResume};
use crate::engine::agent_session::spawn::{spawn_or_resume, ResumeAttempt};
use crate::engine::agent_session::withdraw::{
    begin_withdraw, finish_withdraw, settle_answered_withdraws, PendingWithdraws,
};
use crate::engine::held_deliveries::anchor_is_child_report;
use crate::engine::question_flushes::LoopEndsFlushes;
use crate::engine::session_follow_ups::{drain_lost_followups, lost_followups_to_orphans};
use crate::engine::session_queries::{
    change_description_fallback, default_claude_config_dir, live_claude_config_dir,
};
use crate::engine::types::WithdrawInputRequest;

fn release_tool_slot(tools_in_flight: &std::sync::atomic::AtomicI32) {
    let prev = tools_in_flight.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    if prev <= 0 {
        tools_in_flight.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

fn result_text_is_own_prose(text: &str, cc_error: Option<&str>) -> bool {
    let text = text.trim();
    !text.is_empty() && cc_error.map(str::trim) != Some(text)
}

/// INVARIANT: Wait until the killed agent's process group is gone. The driver drops its
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
            Ok(Some(_)) => continue,
            Ok(None) => return,
            Err(_) => {
                log_legacy!(
                    "[AgentSession] teardown wait after {} timed out for thread {}; the old process may briefly linger",
                    why,
                    thread_id
                );
                return;
            }
        }
    }
}

#[derive(Debug, Default)]
struct StartupDeath {
    reason: Option<String>,
    exit: AgentExitFacts,
}

#[derive(Debug, Default)]
pub(crate) struct InputGate {
    handed_off: bool,
    provider_blocked: bool,
    ceiling_owed: bool,
}

impl InputGate {
    fn receives(&self) -> bool {
        !self.handed_off && !self.provider_blocked
    }

    fn forwards(&self) -> bool {
        self.receives() && !self.ceiling_owed
    }
}

#[derive(Debug, Default)]
struct CeilingDelivery {
    pending: Option<(Uuid, Uuid)>,
    read_in_turn: Option<bool>,
}

impl CeilingDelivery {
    fn observe(&mut self, event: &AgentEvent, started_turn: bool) {
        match event {
            AgentEvent::InputRead { input } | AgentEvent::InputRejected { input }
                if self.pending.is_some_and(|(uuid, _)| uuid == *input) =>
            {
                self.pending = None;
                self.read_in_turn = Some(started_turn);
            }
            _ => {}
        }
    }

    fn turn_ended(&mut self) -> Option<bool> {
        self.read_in_turn.take()
    }
}

fn forward_ceiling_input(
    reentry: AgentUserInput,
    inputs: &mut InputLedger,
    delivery: &mut CeilingDelivery,
    tx: &tokio::sync::mpsc::UnboundedSender<AgentInput>,
) -> bool {
    let Some(anchor) = reentry.origin_event_id else {
        return false;
    };
    let input = AgentInput {
        defer_on_steer_failure: true,
        text: reentry.text,
        images: Vec::new(),
        uuid: Uuid::new_v4(),
    };
    delivery.pending = Some((input.uuid, anchor));
    inputs.forwarded(vec![anchor], &input, None);
    tx.send(input).is_ok()
}

/// INVARIANT: The coding-agent driver dropped its input receiver before the engine could
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

fn unregistered_lucidos_root(
    dev_root: PathBuf,
    has_lucidos_source: bool,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if has_lucidos_source {
        return Ok(dev_root);
    }
    Err(crate::engine::agent_session::err_no_lucidos_source())
}

#[derive(Debug)]
pub(crate) struct ReportedUsage {
    billing: BillingAttribution,
    restored: Option<crate::runtime::agent_runtime::CodexUsageSnapshot>,
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

impl ReportedUsage {
    fn unscoped(snapshot: crate::runtime::agent_runtime::CodexUsageSnapshot) -> Self {
        Self {
            restored: None,
            ..Self::restored(snapshot)
        }
    }

    fn restored(snapshot: crate::runtime::agent_runtime::CodexUsageSnapshot) -> Self {
        Self {
            billing: BillingAttribution::default(),
            input_tokens: snapshot.input_tokens,
            output_tokens: snapshot.output_tokens,
            cache_read_tokens: snapshot.cache_read_tokens,
            cache_creation_tokens: snapshot.cache_creation_tokens,
            cache_creation_1h_tokens: None,
            parent_tool_use_id: None,
            api_call_id: None,
            duration_ms: None,
            reasoning_effort: None,
            restored: Some(snapshot),
        }
    }
}

/// INVARIANT: Bound the stop-time usage drain. Claude Code flushes ahead of group teardown;
/// Codex can still send a usage notification already being mapped at the stop.
const USAGE_FLUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

async fn usage_flushed_at_stop(
    events_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    wait: std::time::Duration,
    billing: &mut BillingAttribution,
) -> Vec<(Option<String>, ReportedUsage)> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut flushed = Vec::new();
    loop {
        let event = match tokio::time::timeout_at(deadline, events_rx.recv()).await {
            Ok(event) => event,
            Err(_) => {
                log_legacy!(
                    "[AgentSession] stop-time usage drain reached its {}s deadline",
                    wait.as_secs()
                );
                break;
            }
        };
        match event {
            Some(AgentEvent::Usage {
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
            }) => flushed.push((
                model,
                ReportedUsage {
                    billing: billing.clone(),
                    restored: None,
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
            Some(AgentEvent::UsageRestored { snapshot }) => {
                flushed.push((None, ReportedUsage::restored(snapshot)));
            }
            Some(AgentEvent::UsageUnscoped { snapshot }) => {
                flushed.push((None, ReportedUsage::unscoped(snapshot)));
            }
            Some(AgentEvent::BillingObserved { attribution }) => *billing = attribution,
            Some(AgentEvent::OutputEnded | AgentEvent::Exited { .. }) | None => break,
            Some(_) => {}
        }
    }
    flushed
}

#[derive(Debug, PartialEq)]
pub(crate) struct CallModels {
    model: String,
    served_model: Option<String>,
}

impl CallModels {
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

fn effort_in_force(
    coding_agent: CodingAgent,
    spawn_effort: Option<&str>,
    reported_effort: Option<String>,
) -> Option<String> {
    match coding_agent {
        CodingAgent::ClaudeCode => spawn_effort.map(String::from),
        CodingAgent::Codex => reported_effort,
    }
}

fn coding_agent_capture(
    coding_agent: CodingAgent,
    models: CallModels,
    context_window: usize,
    reported: ReportedUsage,
    reasoning_effort: Option<String>,
) -> crate::engine::thread_events::ThreadEvent {
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
        trim_passes: Vec::new(),
        purpose: crate::engine::ContextPurpose::Turn,
        reconstructed: false,
        parent_tool_use_id: reported.parent_tool_use_id,
        api_call_id: reported.api_call_id,
        reasoning_effort,
        duration_ms: reported.duration_ms,
        served_model: models.served_model,
        provider: Some(reported.billing.provider_id().to_string()),
        stable_prefix_fingerprint: None,
        ttft_ms: None,
        request_id: None,
        telemetry_owned_by_cli: false,
        session: None,
    }
}

#[easy_ext::ext(RunAgentSessionExt)]
pub(crate) impl LucidosEngine {
    async fn observe_coding_agent_billing(
        &self,
        thread_id: Uuid,
        coding_agent: CodingAgent,
        launch_id: Uuid,
        attribution: &BillingAttribution,
        meta: &crate::engine::thread_events::EventMeta,
    ) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentBillingObserved {
                        coding_agent,
                        launch_id,
                        attribution: attribution.clone(),
                        billing_mode: attribution.billing_mode(),
                    },
                    meta: meta.clone(),
                },
                "[AgentSession] CodingAgentBillingObserved",
            )
            .await;
    }

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
        launch_id: Uuid,
    ) {
        let restored_provider = if let Some(snapshot) = &reported.restored {
            super::resume_billing::restored_provider(
                &self.event_store,
                thread_id,
                launch_id,
                snapshot,
            )
            .await
        } else {
            None
        };
        let restored = reported.restored.is_some();
        let reasoning_effort = effort_in_force(
            coding_agent,
            if restored { None } else { spawn_effort },
            reported.reasoning_effort.take(),
        );
        let main_capture = !restored && reported.parent_tool_use_id.is_none();
        if main_capture {
            super::completion::note_effort_in_force(
                &mut *self.agent_sessions.lock().await,
                thread_id,
                token,
                reasoning_effort.as_deref(),
            );
        }
        let context_window =
            crate::runtime::coding_agent_context_window(coding_agent, &models.model)
                .unwrap_or_else(|| self.context_window_for(&models.model, None));
        let mut capture = coding_agent_capture(
            coding_agent,
            models,
            context_window,
            reported,
            reasoning_effort,
        );
        if restored {
            if let crate::engine::thread_events::ThreadEvent::ContextCaptured { provider, .. } =
                &mut capture
            {
                *provider = Some(restored_provider.unwrap_or_else(|| "unknown".to_string()));
            }
        }
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: capture,
                    meta: meta.clone(),
                },
                "[AgentSession] ContextCaptured",
            )
            .await;
        if main_capture {
            if let Err(e) =
                crate::engine::context_ceiling::record_capture_crossing(&self.event_bus, thread_id)
                    .await
            {
                log_legacy!(
                    "[AgentSession] Could not record context ceiling crossing for {}: {}",
                    thread_id,
                    e
                );
            }
        }
    }

    async fn idle_exit_ends_session(
        &self,
        thread_id: Uuid,
        msg_rx: &tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
        owed: u32,
        agent_cancel: &tokio_util::sync::CancellationToken,
        action: IdleAction,
        input_gate: &mut InputGate,
    ) -> bool {
        if action == IdleAction::Nothing {
            return false;
        }
        let exited = self
            .exit_idle_subprocess_unless_owed(thread_id, msg_rx, owed, agent_cancel, input_gate)
            .await;
        exited && action == IdleAction::EndSession
    }

    async fn exit_idle_subprocess_unless_owed(
        &self,
        thread_id: Uuid,
        msg_rx: &tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
        owed: u32,
        agent_cancel: &tokio_util::sync::CancellationToken,
        input_gate: &mut InputGate,
    ) -> bool {
        input_gate.handed_off =
            crate::engine::pending_handoff::handoff_pending(self.pool(), thread_id).await;
        let leaves_queue_to_next_session = !input_gate.forwards();
        let mut sessions = self.agent_sessions.lock().await;
        let redirect_pending = sessions
            .get_mut(&thread_id)
            .map(|s| std::mem::take(&mut s.redirect_followup_pending))
            .unwrap_or(false);
        match terminate_decision(
            msg_rx.len(),
            owed,
            redirect_pending,
            leaves_queue_to_next_session,
        ) {
            TerminateDecision::KeepAliveForFollowup {
                queued,
                unread,
                redirect_pending,
            } => {
                log_legacy!("[AgentSession] Skipping subprocess termination for thread {}: a follow-up is still on its way ({} queued, {} unread, redirect pending: {})", thread_id, queued, unread, redirect_pending);
                false
            }
            TerminateDecision::Terminate => {
                if let Some(s) = sessions.get_mut(&thread_id) {
                    s.process_exited = true;
                    s.idle_notify.notify_waiters();
                }
                log_legacy!("[AgentSession] Idle reached — terminating the coding-agent subprocess for thread {} so next turn resumes via --resume", thread_id);
                agent_cancel.cancel();
                true
            }
        }
    }

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
        billing: &mut BillingAttribution,
        launch_id: Uuid,
    ) {
        let mut recorded_billing = billing.clone();
        let flushed = usage_flushed_at_stop(events_rx, USAGE_FLUSH_WAIT, billing).await;
        for (model, reported) in flushed {
            if reported.restored.is_none() && reported.billing != recorded_billing {
                self.observe_coding_agent_billing(
                    thread_id,
                    coding_agent,
                    launch_id,
                    &reported.billing,
                    meta,
                )
                .await;
                recorded_billing = reported.billing.clone();
            }
            self.record_coding_agent_usage(
                thread_id,
                token,
                meta,
                coding_agent,
                CallModels::of(session_model, model.as_deref()),
                reported,
                spawn_effort,
                launch_id,
            )
            .await;
        }
        if *billing != recorded_billing {
            self.observe_coding_agent_billing(thread_id, coding_agent, launch_id, billing, meta)
                .await;
        }
    }

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

    #[allow(clippy::too_many_arguments)]
    async fn run_direct_agent(
        &self,
        claim: &mut crate::engine::session_claim::SpawnClaim,
        request_id: Uuid,
        user_message: &str,
        user_images: Option<&[crate::engine::types::ChatImage]>,
        origin_id: Uuid,
        carried_event_ids: &[Uuid],
        spawning_event_id: Option<Uuid>,
        cancel_token: &tokio_util::sync::CancellationToken,
        conflict: Option<&crate::engine::change_ops::apply_claim::ApplyClaim>,
        recovery_worktree: Option<(PathBuf, String)>,
        repo_id: Option<String>,
        system_prompt_override: Option<String>,
        resume_session_id: Option<String>,
        cc_model: Option<String>,
        cc_reasoning_effort: Option<String>,
        resume_worktree_path: Option<PathBuf>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        require_agent_input(user_message, user_images)?;
        let thread_id = claim.thread_id;
        let lane_duty = crate::engine::landing_lane::lane_duty_for_run(
            self.pool(),
            self.workspace_path(),
            self.repo_root(),
            thread_id,
            origin_id,
        )
        .await?;
        let lane_duty = if conflict.is_none() && recovery_worktree.is_none() {
            lane_duty
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
        claim: &mut crate::engine::session_claim::SpawnClaim,
        request_id: Uuid,
        user_message: &str,
        user_images: Option<&[crate::engine::types::ChatImage]>,
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
        self.gate_coding_agent_spawn(thread_id, origin_id).await?;
        if claim.entry.agent_cancel.is_cancelled() {
            claim.entry.agent_cancel = tokio_util::sync::CancellationToken::new();
        }
        let (model, effort) = self
            .coding_agent_rung(
                thread_id,
                claim.entry.coding_agent,
                cc_model,
                cc_reasoning_effort,
            )
            .await;
        let records_rung = !self.event_bus.in_keep_alive_turn(thread_id);
        let (cc_model, pinned_effort) = {
            let mut sessions = self.agent_sessions.lock().await;
            let session = sessions
                .get_mut(&thread_id)
                .filter(|s| s.msg_tx.same_channel(&claim.entry.msg_tx));
            if let Some(session) = session {
                session.agent_cancel = claim.entry.agent_cancel.clone();
                session.current_model = model.clone();
                session.current_reasoning_effort = effort.clone();
            }
            (model, effort)
        };
        let conflict_change_id = conflict.map(|c| c.change_id());
        let is_resolution_session = conflict_change_id.is_some() || lane_duty.is_some();
        let thread_id_str = thread_id.to_string();
        let coding_agent = claim.entry.coding_agent;
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
                        log_legacy!(
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
        let integrated =
            crate::engine::integrated::read_thread_is_integrated(&self.pool, thread_id).await?;

        let mut meta = crate::engine::thread_events::EventMeta {
            request_event_id: Some(origin_id),
            channel: Some(EventChannel::ClaudeCode),
            ..crate::engine::thread_events::EventMeta::NONE
        };

        let recorded_pin =
            crate::engine::agent_session::lookup_pinned_cc_config_dir(self.pool(), thread_id)
                .await?;

        let _awake = crate::core::keep_awake::hold(
            crate::core::keep_awake::Work::AgentSession,
            thread_id.to_string(),
        );

        let run_kind = if recovery_worktree.is_some() || conflict_change_id.is_some() {
            RunKind::Resolution
        } else if lane_duty.is_some() {
            RunKind::Lane
        } else {
            RunKind::Ordinary
        };
        let run_resume = resolve_run_resume(
            self.pool(),
            self.changes(),
            thread_id,
            run_kind,
            resume_session_id,
            recorded_pin.as_ref().map(|pin| pin.dir.as_str()),
        )
        .await?;
        let records_continuity = run_resume.records_continuity(run_kind);
        let RunResume {
            pending_handoff,
            blocking_policy,
            session_id: resume_session_id,
            branch: resume_branch,
        } = run_resume;

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

        log_legacy!(
            "[AgentSession] [TIMING] resume lookup: {:?}",
            cc_start.elapsed()
        );

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

        let dev_root = main_worktree().await;

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
            (
                None,
                unregistered_lucidos_root(dev_root, crate::paths::has_lucidos_source())?,
                false,
                None,
                None,
            )
        };
        let session_kind = if is_app_spawn {
            crate::engine::thread_events::CodingAgentKind::App
        } else if is_external_repo {
            crate::engine::thread_events::CodingAgentKind::External
        } else {
            crate::engine::thread_events::CodingAgentKind::Lucidos
        };

        let workspace_name = self.workspace_name();
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

        let adoption_anchor_sha = match worktree_path.as_deref() {
            Some(wt) => crate::engine::agent_session::external_edits::git_head_sha(wt)
                .await
                .or_else(|| last_idle_sha.clone()),
            None => last_idle_sha.clone(),
        };

        if integrated && !is_resolution_session {
            session_context.push(
                &crate::engine::agent_session::prompts::integrated_section(self.pool(), thread_id)
                    .await,
            );
        }

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
        if pending_handoff.is_none() {
            let thread_messages = self.event_store.get_thread_messages(&thread_id_str).await?;
            let in_first_prompt: Vec<String> =
                crate::engine::agent_session::turn_gap::compute_turn_gap_note(
                    &self.pool, thread_id, origin_id, None,
                )
                .await
                .map(|note| note.user_event_ids.iter().map(Uuid::to_string).collect())
                .unwrap_or_default();
            let users = crate::gateway_users::current();
            let (prior, current_at) = before_current_message(
                thread_messages
                    .iter()
                    .filter(|m| {
                        m.event_id
                            .as_ref()
                            .is_none_or(|id| !in_first_prompt.contains(id))
                    })
                    .map(|m| HistoryLine::of(m, &users))
                    .collect(),
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
                        images.push(crate::engine::types::ChatImage { base64, mime_type });
                    }
                    None => log_legacy!(
                        "[AgentSession] Image {} of thread {} is gone; the fresh session goes without it",
                        hash,
                        thread_id
                    ),
                }
            }
            session_context.push(&thread_history_block(&prior, &attached));
            session_context.attach_images(images);
        }
        let opened_message = match (&pending_handoff, blocking_policy) {
            (Some(handoff), _) => Some(handoff.first_message(user_message)),
            (None, Some(policy)) => Some(crate::engine::successor::provider_blocked_input(
                thread_id,
                policy,
                user_message,
            )),
            (None, None) => None,
        };
        let first_message = opened_message.as_deref().unwrap_or(user_message);
        let first_prompt = if self.event_bus.in_keep_alive_turn(thread_id) {
            PromptFor::KeepAlive(first_message)
        } else {
            PromptFor::Turn(first_message)
        };
        let session_context = session_context.render();

        let startup_cancel = claim.entry.startup_cancel.clone();
        let startup_permit = tokio::select! {
            biased;
            _ = startup_cancel.cancelled() => None,
            permit = self.cc_startup_semaphore.clone().acquire_owned() => {
                Some(permit.map_err(|e| format!("Startup semaphore closed: {}", e))?)
            }
        };

        if resume_session_id.is_some() {
            log_legacy!("[AgentSession] Resuming session for thread {}", thread_id);
        }
        let agent_cancel = claim.entry.agent_cancel.clone();
        let allowed_tools = crate::engine::claude_code::cc_allowed_tools(&self.event_bus).await;
        let user_env_vars =
            crate::core::EnvironmentVariableStore::spawn_pairs(&self.pool, "AgentSession").await;
        let build_slot_class =
            crate::engine::main_check::repair_build_class(self.pool(), thread_id)
                .await
                .map_err(|error| format!("Could not read repair build class: {error}"))?;
        let tool_result_secrets =
            crate::runtime::spawn_env::session_secret_values(thread_id, &user_env_vars);
        let injected_pin = recorded_pin
            .as_ref()
            .map(|recorded| recorded.resolve(&user_env_vars));
        let live_account = crate::runtime::AccountPin::for_first_session(
            live_claude_config_dir(&user_env_vars),
            default_claude_config_dir(),
        );
        let effective_pin = injected_pin.clone().or_else(|| live_account.clone());
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
        let cc_default_model = cc_defaults.as_ref().and_then(|s| s.default_model());
        let binary_override_pref = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => {
                &crate::core::prefs::CODING_AGENT_CLAUDE_PATH
            }
            crate::runtime::CodingAgent::Codex => &crate::core::prefs::CODING_AGENT_CODEX_PATH,
        };
        let binary_override = binary_override_pref.read(self.pool()).await;
        let notes = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => {
                crate::runtime::claude_code_notes::notes_visibility(cc_model.as_deref())
            }
            crate::runtime::CodingAgent::Codex => {
                crate::runtime::claude_code_notes::NotesVisibility::Shown
            }
        };
        let system_prompt =
            crate::engine::agent_session::prompts::with_notes_visibility(system_prompt, notes);
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
        log_legacy!(
            "[AgentSession] [TIMING] Startup semaphore acquired: {:?}",
            cc_start.elapsed()
        );
        let sandbox =
            crate::runtime::agent_sandbox::Sandbox::resolve(self.pool(), self.workspace_path())
                .await;
        let sandbox_mode = sandbox.as_ref().ok().map(|sandbox| sandbox.mode());
        let execution_sandbox = sandbox.as_ref().ok().cloned();
        let spawned = match sandbox {
            Ok(sandbox) => {
                spawn_or_resume(
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
                        interactive: interactive_session && !integrated,
                        build_slot_class,
                        user_env_vars: &user_env_vars,
                        binary_override: binary_override.as_deref(),
                        permission_mode: permission_mode.as_deref(),
                        account_pin: injected_pin.as_ref(),
                        additional_directories: &repo_directory_grants,
                        auto_compact,
                        keep_alive: matches!(first_prompt, PromptFor::KeepAlive(_)),
                        agent_run: crate::runtime::agent_run_marker::AgentRun::for_thread(
                            self.workspace_path(),
                            thread_id,
                            self.teardown_handover.clone(),
                        ),
                        sandbox,
                    },
                    agent_cancel.clone(),
                )
                .await
            }
            Err(e) => Err(e),
        };
        let (runtime, resume_attempt) = match spawned {
            Ok(r) => r,
            Err(e) => {
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

        log_legacy!(
            "[AgentSession] [TIMING] CC process spawned: {:?}",
            cc_start.elapsed()
        );

        let crate::runtime::RunningAgent {
            mut events_rx,
            input_tx: agent_input_tx,
            control_tx: agent_control_tx,
            withdraw_tx: agent_withdraw_tx,
            kind: _,
            permission_rx: mut agent_permission_rx,
        } = runtime;

        let images = user_images.map(|imgs| imgs.to_vec()).unwrap_or_default();

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
            defer_on_steer_failure: false,
            text: first.text,
            images,
            uuid: Uuid::new_v4(),
        };
        let mut blocks_in_flight: HashMap<Uuid, ContextFingerprint> = HashMap::new();
        if let Some(fingerprint) = first.device_block {
            blocks_in_flight.insert(first_input.uuid, fingerprint);
        }
        let mut inputs = InputLedger::new();
        inputs.forwarded(first_prompt_events, &first_input, None);
        if agent_input_tx.send(first_input).is_err() {
            crate::engine::git_ops::cleanup_failed_spawn(
                &repo_root,
                worktree_path.as_deref(),
                &branch_name,
                worktree_created,
                branch_created,
            )
            .await;
            let death = drain_startup_death(&mut events_rx);
            return Err(
                match classify_startup_failure(resume_session_id.is_some(), death.reason.clone()) {
                    StartupFailure::Failed(plain) => {
                        startup_failure_error(coding_agent, death, plain)
                    }
                    StartupFailure::StaleResume => {
                        log_legacy!(
                            "[AgentSession] thread {} could not resume sid={}: the coding agent has no such session. Retrying with a fresh one.",
                            thread_id,
                            resume_session_id.as_deref().unwrap_or("")
                        );
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
        let mut withdraws_awaiting_forward: Vec<WithdrawInputRequest> = Vec::new();
        let stop = claim.entry.stop.clone();
        let interrupt = claim.entry.interrupt.clone();
        let idle_notify = claim.entry.idle_notify.clone();
        let shutting_down = claim.entry.shutting_down.clone();
        let external_terminal_emitted = claim.entry.external_terminal_emitted.clone();
        let external_continuation_requested = claim.entry.external_continuation_requested.clone();
        let mut normalized_model = cc_model.clone();
        let mut rung_effort = cc_reasoning_effort.clone();
        let mut pending_model_swap: Option<crate::engine::model_swap::ModelSwap> = None;
        let tools_in_flight_shared = claim.entry.tools_in_flight.clone();
        let question_flushes = LoopEndsFlushes(claim.entry.question_flushes.clone());

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
                session.conflict = conflict_change_id
                    .map(|change_id| crate::engine::types::ConflictBinding::Detached { change_id })
                    .or_else(|| {
                        lane_duty.as_ref().map(|duty| {
                            crate::engine::types::ConflictBinding::LaneBatch {
                                batch_id: duty.batch_id,
                            }
                        })
                    });
                session.execution_sandbox = execution_sandbox.clone();
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
            agent_cancel.cancel();
            await_process_teardown(&mut events_rx, thread_id, "a Stop during startup").await;
            return Ok(self.settle_canceled_startup(claim, request_id, &meta).await);
        }
        claim.entry_guard.settle_if_dropped();
        if let Some(apply_claim) = conflict {
            apply_claim.carried_by_session(external_continuation_requested.clone());
        }
        let msg_tx = claim.entry.msg_tx.clone();
        let crate::engine::session_claim::ClaimedEntry {
            msg_rx,
            withdraw_rx,
            child_reports,
            ..
        } = &mut claim.entry;

        self.redeliver_stranded_follow_ups(thread_id, &msg_tx, msg_rx, stranded)
            .await;
        if !self.event_bus.in_keep_alive_turn(thread_id) {
            let held = crate::engine::thread_stop::held_inputs(self.pool(), thread_id).await?;
            if let Some(session) = self.agent_sessions.lock().await.get_mut(&thread_id) {
                crate::engine::thread_stop::queue_held_inputs(session, held, |id| {
                    inputs.forwarded_here(id)
                });
            }
        }

        let chat_cancel = cancel_token.clone();
        let images: Vec<String> = Vec::new();

        // INVARIANT: Emit SessionStarted immediately so the branch→thread mapping exists
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
            repo_root.to_string_lossy().to_string()
        };
        // INVARIANT: Read before the emit below, which would count as an earlier session.
        let continuity = if records_continuity || resume_attempt == ResumeAttempt::Resumed {
            resolve_session_continuity(
                self.pool(),
                thread_id,
                resume_attempt,
                branch_gone,
                blocking_policy.is_some(),
                pending_handoff.as_ref(),
            )
            .await
        } else {
            None
        };
        let session_start = match self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::SessionStarted {
                    sandbox: sandbox_mode,
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
            Ok(event) => event.map(|event| event.seq),
            Err(e) => {
                log_legacy!(
                    "[AgentSession] Failed to emit initial SessionStarted for {}: {}",
                    thread_id,
                    e
                );
                None
            }
        };

        crate::engine::session_seed::seed_branch_work(
            self.pool(),
            thread_id,
            &repo_root,
            &branch_name,
        )
        .await;

        if records_rung && (normalized_model.is_some() || cc_reasoning_effort.is_some()) {
            if let Err(e) = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentSettingsChanged {
                        model: normalized_model.clone(),
                        reasoning_effort: cc_reasoning_effort.clone(),
                        coding_agent,
                        cc_session_id: None,
                        claude_config_dir: None,
                        claude_config_dir_explicit: None,
                    },
                    meta: meta.clone(),
                })
                .await
            {
                log_legacy!(
                    "[AgentSession] Failed to persist initial CodingAgentSettingsChanged for {}: {}",
                    thread_id,
                    e
                );
            }
        }

        let mut result_texts: Vec<String> = Vec::new();
        let mut tool_calls_seen: u32 = 0;
        let mut api_calls_seen: u32 = 0;
        let mut init_session_id: Option<String> = None;
        let billing_launch_id = Uuid::new_v4();
        let mut billing_attribution = BillingAttribution::default();
        self.observe_coding_agent_billing(
            thread_id,
            coding_agent,
            billing_launch_id,
            &billing_attribution,
            &meta,
        )
        .await;
        let mut claude_text_buf = CodingAgentTextBuffer::default();
        let mut claude_thought_buf = String::new();
        let mut last_thought_persisted_len: usize = 0;
        const THOUGHT_FLUSH_THRESHOLD: usize = 240;
        let mut is_waiting = false;
        let mut input_gate = InputGate::default();
        let mut proposed_change = false;
        let mut reentry_ledger = crate::engine::turn_end::ReentryLedger::default();
        let mut ceiling_reentry_forwarded = false;
        let mut ceiling_delivery = CeilingDelivery::default();
        let mut approved_ceiling_reentry = None;
        let mut held_ceiling_inputs = Vec::new();
        let mut emitted_terminal_event = false;
        let mut user_hit_stop = false;
        let mut redirect_cause = None;
        let mut last_emitted_idle = false;
        let tools_in_flight = tools_in_flight_shared;
        let mut tool_names = super::super::tool_output::ToolNamesById::default();
        let mut last_terminal_kind: Option<TerminalKind> = None;
        let mut withheld_api_error: Option<String> = None;
        let mut watchdog_fired = false;
        let mut exit_facts = AgentExitFacts::default();
        let mut interrupt_escalate_at: Option<tokio::time::Instant> = None;
        const INTERRUPT_ESCALATE_AFTER: std::time::Duration = std::time::Duration::from_secs(8);

        'event_loop: loop {
            let escalate_deadline = interrupt_escalate_at;
            tokio::select! {
                event_opt = events_rx.recv() => {
                    let Some(ev) = event_opt else {
                        log_legacy!(
                            "[AgentSession] events_rx closed without AgentEvent::Exited for thread {}",
                            thread_id
                        );
                        break;
                    };
                    if matches!(ev, AgentEvent::OutputEnded) {
                        continue;
                    }
                    if let AgentEvent::BillingObserved { attribution } = &ev {
                        billing_attribution = attribution.clone();
                        self.observe_coding_agent_billing(
                            thread_id, coding_agent, billing_launch_id, &billing_attribution, &meta,
                        ).await;
                        continue;
                    }
                    if let AgentEvent::UsageRestored { snapshot } = ev {
                        self.record_coding_agent_usage(
                            thread_id, &external_terminal_emitted, &meta, coding_agent,
                            CallModels::of(normalized_model.as_deref(), None),
                            ReportedUsage::restored(snapshot), None, billing_launch_id,
                        ).await;
                        continue;
                    }
                    if let AgentEvent::UsageUnscoped { snapshot } = ev {
                        api_calls_seen = api_calls_seen.saturating_add(1);
                        self.record_coding_agent_usage(
                            thread_id, &external_terminal_emitted, &meta, coding_agent,
                            CallModels::of(normalized_model.as_deref(), None),
                            ReportedUsage::unscoped(snapshot), None, billing_launch_id,
                        ).await;
                        continue;
                    }
                    if let AgentEvent::SteerFallback { can_steer, interrupting } = ev {
                        if let Some(session) = self.agent_sessions.lock().await.get_mut(&thread_id) {
                            session.can_steer = can_steer;
                        }
                        if interrupting && !is_waiting {
                            user_hit_stop = true;
                            redirect_cause = Some(crate::engine::thread_events::CancelCause::SupersededByFollowup);
                            interrupt_escalate_at = Some(tokio::time::Instant::now() + INTERRUPT_ESCALATE_AFTER);
                        }
                        continue;
                    }
                    settle_answered_withdraws(&self.event_bus, thread_id, &mut inputs, &mut pending_withdraws).await;
                    let (read_owed_input, reads) = match inputs.observe(&ev) {
                        Settled::Read(reads) => (true, reads),
                        Settled::Rejected => {
                            log_legacy!("[AgentSession] The agent rejected a forwarded input on thread {}; it stays unread", thread_id);
                            (false, Vec::new())
                        }
                        Settled::Nothing => (false, Vec::new()),
                    };
                    child_reports.publish(inputs.owed_child_reports());
                    if let AgentEvent::Exited { exit } = ev {
                        exit_facts = exit;
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
                            // INVARIANT: The process exited after producing a Result, so the
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
                                log_legacy!("[AgentSession] CC process exited while idle with a stop pending for thread {}; finalize carries it out", thread_id);
                                break;
                            };
                            log_legacy!("[AgentSession] CC process exited while idle, releasing thread {}", thread_id);
                            let cache_snapshot = entry.and_then(|s| {
                                s.repo_root.as_ref().map(|r| {
                                    (r.to_string_lossy().to_string(), s.to_commands_info())
                                })
                            });
                            if let Some((repo_key, info)) = cache_snapshot {
                                self.upsert_cc_commands_cache(repo_key, info).await;
                            }

                            for held in held_ceiling_inputs.drain(..).rev() {
                                requeue_at_head(&msg_tx, msg_rx, held);
                            }
                            let orphans = lost_followups_to_orphans(drain_lost_followups(msg_rx));

                            self.maybe_auto_resume_after_api_error(
                                thread_id,
                                &meta,
                                withheld_api_error.take(),
                                !orphans.is_empty(),
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
                        log_legacy!(
                            "[AgentSession] CC exited without Result event for thread {} (buffered_text_len={})",
                            thread_id,
                            claude_text_buf.as_str().len()
                        );
                        break;
                    }
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
                                s.is_waiting = false;
                            }
                            reset
                        } else {
                            false
                        }
                    };
                    if new_turn_after_terminal {
                        log_legacy!(
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
                    ceiling_delivery.observe(&ev, new_turn_after_terminal);
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
                        AgentEvent::SteerFallback { .. } | AgentEvent::BillingObserved { .. } | AgentEvent::UsageRestored { .. } | AgentEvent::UsageUnscoped { .. } => unreachable!("billing and snapshots handled before activity"),
                        AgentEvent::Init { session_id: cc_sid, model: init_model, slash_commands: cmds, skills, agent_version, can_steer } => {
                            log_legacy!("[AgentSession] [TIMING] Init event received: {:?}", cc_start.elapsed());
                            let first_init = init_session_id.replace(cc_sid.clone()).is_none();
                            let requested_model = cc_model.as_deref().or(cc_default_model.as_deref());
                            let substitution = init_model
                                .as_deref()
                                .filter(|_| first_init && coding_agent == CodingAgent::ClaudeCode)
                                .and_then(|m| crate::runtime::claude_code::substituted_cc_model(requested_model, m))
                                .zip(requested_model.map(String::from));
                            let mut model_swap = None;
                            if let Some((served, requested)) = &substitution {
                                let served = crate::runtime::claude_code::reconcile_cc_model(
                                    requested_model,
                                    served,
                                );
                                let requested_effort = match rung_effort.take() {
                                    Some(effort) => Some(effort),
                                    None => self.swap_fallback_effort(coding_agent, requested).await,
                                };
                                rung_effort = self.swap_fallback_effort(coding_agent, &served).await;
                                model_swap = Some(crate::engine::model_swap::ModelSwap {
                                    requested: crate::runtime::intelligence_ladder::IntelligenceRung::new(
                                        requested,
                                        &requested_effort.unwrap_or_default(),
                                    ),
                                    served,
                                    served_effort: rung_effort.clone(),
                                });
                            }
                            let cache_update = {
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) {
                                    s.backend_session_id = Some(cc_sid.clone());
                                    s.can_steer = can_steer;
                                    if substitution.is_some() {
                                        s.current_reasoning_effort = rung_effort.clone();
                                    }
                                    if let Some(ref m) = init_model {
                                        let norm = crate::runtime::claude_code::reconcile_cc_model(
                                            requested_model,
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
                                self.refresh_cc_models_if_stale(
                                    agent_version.as_deref(),
                                    live_account.as_ref(),
                                );
                                if first_init {
                                    self.observe_cc_live_account(live_account.as_ref());
                                }
                            }
                            if let Some(info) = cache_update {
                                let repo_key = repo_root.to_string_lossy().to_string();
                                self.upsert_cc_commands_cache(repo_key, info).await;
                            }
                            if let Err(e) = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentSettingsChanged {
                                    model: normalized_model.clone().filter(|_| records_rung),
                                    reasoning_effort: rung_effort.clone().filter(|_| records_rung),
                                    coding_agent,
                                    cc_session_id: Some(cc_sid.clone()),
                                    claude_config_dir: effective_pin.as_ref().map(|pin| pin.dir().to_string()),
                                    claude_config_dir_explicit: effective_pin.as_ref().map(|pin| pin.is_explicit()),
                                },
                                meta: meta.clone(),
                            }).await {
                                log_legacy!("[AgentSession] Failed to persist Init CodingAgentSettingsChanged for {}: {}", thread_id, e);
                            }
                            let served_as_asked = first_init
                                && coding_agent == CodingAgent::ClaudeCode
                                && init_model.is_some();
                            if let (None, true, Some(requested)) = (&substitution, served_as_asked, requested_model) {
                                self.record_cc_model_served(
                                    requested,
                                    effective_pin.as_ref(),
                                    live_account.as_ref(),
                                );
                            }
                            if let Some((served, requested)) = substitution {
                                if self.record_cc_model_refusal(
                                    &requested,
                                    agent_version.as_deref(),
                                    effective_pin.as_ref(),
                                    live_account.as_ref(),
                                ) {
                                    pending_model_swap = model_swap;
                                }
                                self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentModelSubstituted {
                                        requested,
                                        served,
                                        coding_agent,
                                    },
                                    meta: meta.clone(),
                                }, "[AgentSession] CodingAgentModelSubstituted").await;
                            }
                            if let Some(permit) = startup_permit.take() {
                                drop(permit);
                                log_legacy!("[AgentSession] [TIMING] Startup semaphore released: {:?}", cc_start.elapsed());
                            }
                        }
                        AgentEvent::Message { text, .. } | AgentEvent::ProgressNote { text }
                            if turn_ended =>
                        {
                            log_legacy!(
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
                        AgentEvent::Thought { .. } if turn_ended => {}
                        AgentEvent::Thought { text } => {
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            claude_thought_buf.push_str(&text);
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
                        AgentEvent::ToolUse { name, id, input, .. } if turn_ended => {
                            if crate::runtime::is_user_question_tool(&name) {
                                question_flushes.0.flushed(&id, &name, &input);
                            }
                            log_legacy!(
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
                            tools_in_flight.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            tool_calls_seen = tool_calls_seen.saturating_add(1);
                            tool_names.record(&id, &name);
                            if crate::runtime::is_user_question_tool(&name) {
                                question_flushes.0.flushed(&id, &name, &input);
                            } else {
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
                        AgentEvent::ToolResult { id, .. } if turn_ended => {
                            release_tool_slot(&tools_in_flight);
                            tool_names.take(&id);
                            log_legacy!(
                                "[AgentSession] Dropping post-terminal straggler tool result for thread {} — would resurrect 'running' on an idled thread",
                                thread_id
                            );
                        }
                        AgentEvent::ToolResult { output, status: _, id, parent_tool_use_id } => {
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
                            let main_capture = parent_tool_use_id.is_none();
                            api_calls_seen = api_calls_seen.saturating_add(1);
                            self.record_coding_agent_usage(
                                thread_id,
                                &external_terminal_emitted,
                                &meta,
                                coding_agent,
                                CallModels::of(normalized_model.as_deref(), cc_msg_model.as_deref()),
                                ReportedUsage {
                                    billing: billing_attribution.clone(),
                                    restored: None,
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
                                billing_launch_id,
                            )
                            .await;
                            if main_capture && !is_resolution_session && !self.event_bus.in_keep_alive_turn(thread_id) {
                                match crate::engine::context_ceiling::past_ceiling(self.pool(), thread_id).await {
                                    Ok(owed) => {
                                        input_gate.ceiling_owed = owed.is_some();
                                        if !input_gate.ceiling_owed && !ceiling_reentry_forwarded {
                                            for held in held_ceiling_inputs.drain(..).rev() {
                                                requeue_at_head(&msg_tx, msg_rx, held);
                                            }
                                        }
                                    },
                                    Err(e) => log_legacy!("[AgentSession] Ceiling debt unreadable for {thread_id}: {e}"),
                                }
                                let can_steer = {
                                    let mut sessions = self.agent_sessions.lock().await;
                                    sessions.get_mut(&thread_id).is_some_and(|s| {
                                        s.in_ceiling_reentry = input_gate.ceiling_owed || ceiling_reentry_forwarded;
                                        s.can_steer
                                    })
                                };
                                if input_gate.ceiling_owed && can_steer && !ceiling_reentry_forwarded && !user_hit_stop {
                                    match crate::engine::context_ceiling::ceiling_steer(
                                        &self.event_bus, thread_id, inputs.owed() > 0, can_steer, &mut reentry_ledger, &mut approved_ceiling_reentry,
                                    ).await {
                                        Ok(Some(reentry)) => {
                                            ceiling_reentry_forwarded = forward_ceiling_input(reentry, &mut inputs, &mut ceiling_delivery, &agent_input_tx);
                                            if !ceiling_reentry_forwarded { break 'event_loop; }
                                        }
                                        Ok(None) => {}
                                        Err(e) => log_legacy!("[AgentSession] Ceiling steer not recorded for {thread_id}: {e}"),
                                    }
                                }
                            }
                        }
                        AgentEvent::Compacted { trigger, pre_tokens, post_tokens } => {
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
                        AgentEvent::StreamActivity => {}
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
                        AgentEvent::Result { text, error: cc_error, provider_error, turn_end, .. } => {
                                        let err_suffix = cc_error.as_deref().map(|e| format!(" (error: {})", e)).unwrap_or_default();
                                        log_legacy!("[AgentSession] Result event received — entering waiting state{}", err_suffix);
                                        if provider_error.as_ref().is_some_and(|e| e.is_policy_block()) {
                                            input_gate.provider_blocked = true;
                                        }
                                        self.flush_coding_agent_thought(
                                            thread_id,
                                            &claude_thought_buf,
                                            &mut last_thought_persisted_len,
                                            coding_agent,
                                            &meta,
                                        )
                                        .await;
                                        let result_text_is_live = !external_terminal_emitted
                                            .load(std::sync::atomic::Ordering::Acquire);
                                        if !claude_text_buf.is_empty() {
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
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: text.trim().to_string(), coding_agent, parent_tool_use_id: None, progress_note: false },
                                                meta: meta.clone(),
                                            }, "[AgentSession] CodingAgentTextStreamed (slash command result)").await;
                                        }
                                        let result_text_empty = text.trim().is_empty();
                                        let buffered_text_empty = claude_text_buf.as_str().trim().is_empty();
                                        let explicit_session_not_found = resume_session_id.is_some()
                                            && is_definitive_session_not_found(cc_error.as_deref());
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
                                        if is_resume_settle_result(stale_inputs, api_calls_seen == 0) {
                                            log_legacy!(
                                                "[AgentSession] thread {} resumed sid={} and the backend confirmed the attach, but this Result carries no text, no tool call and no error: it closes the resume-settle turn, not ours. Skipping it and waiting for the real Result.",
                                                thread_id,
                                                resume_session_id.as_deref().unwrap_or("")
                                            );
                                            result_texts.push(text.clone());
                                            continue 'event_loop;
                                        }
                                        if api_calls_seen > 0
                                            && is_resume_settle_result(stale_inputs, true)
                                        {
                                            log_legacy!(
                                                "[AgentSession] thread {} resumed sid={} and produced an empty Result with no tool calls, but {} API call(s) were made: it answers our prompt, not a resume-settle turn. Classifying it.",
                                                thread_id,
                                                resume_session_id.as_deref().unwrap_or(""),
                                                api_calls_seen
                                            );
                                        }
                                        if is_stale_resume_signal(stale_inputs)
                                            || explicit_session_not_found
                                        {
                                            log_legacy!("[AgentSession] Stale resume detected (empty-echo heuristic or explicit session-not-found) — aborting session to retry with a fresh spawn.");
                                            agent_cancel.cancel();
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
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::SessionEnded {
                                                    reason: SessionEndReason::StaleResume,
                                                },
                                                meta: meta.clone(),
                                            }, "[AgentSession] SessionEnded (stale resume)").await;
                                            await_process_teardown(&mut events_rx, thread_id, "a stale resume").await;
                                            return Err(STALE_RESUME_ERROR.into());
                                        }

                                        result_texts.push(text.clone());
                                        let is_shutdown = self.session_is_shutting_down(
                                            shutting_down
                                                .load(std::sync::atomic::Ordering::Relaxed),
                                        );
                                        let quiet_pick = self
                                            .agent_sessions
                                            .lock()
                                            .await
                                            .get_mut(&thread_id)
                                            .and_then(|s| s.quiet_answer_pending.take());
                                        let ended_on_quiet_answer = quiet_pick.is_some();
                                        let (terminal_kind, emit_idle) = classify_result(
                                            user_hit_stop,
                                            redirect_cause,
                                            is_shutdown,
                                            cc_error,
                                            buffered_text_empty && result_text_empty && !ended_on_quiet_answer,
                                        );
                                        let terminal_text = QuietPick::terminal_text(
                                            quiet_pick,
                                            &text,
                                            matches!(terminal_kind, TerminalKind::Generated),
                                        );
                                        last_terminal_kind = Some(terminal_kind.clone());
                                        self.hold_completion_if_api_error_resume(
                                            thread_id,
                                            &last_terminal_kind,
                                            is_shutdown,
                                            is_resolution_session,
                                        )
                                        .await;
                                        let clears = terminal_clears_user_hit_stop(&terminal_kind);
                                        if clears {
                                            user_hit_stop = false;
                                            redirect_cause = None;
                                        }
                                        if !crate::engine::agent_session::runtime_helpers::external_terminal_already_emitted(&self.pool, &external_terminal_emitted, thread_id, meta.request_event_id, is_shutdown, "Result classify").await {
                                            let (model, reasoning_effort) = self
                                                .terminal_model_settings(thread_id, &external_terminal_emitted)
                                                .await;
                                            let mut terminal_event = Self::make_terminal_event(
                                                terminal_kind,
                                                terminal_text,
                                                model,
                                                reasoning_effort,
                                            );
                                            if let crate::engine::thread_events::ThreadEvent::ResponseFailed { spend_cap, provider_error: failed_provider_error, .. } = &mut terminal_event {
                                                if let Some(notice) = spend_cap {
                                                    notice.provider = coding_agent.display_name().to_owned();
                                                }
                                                *failed_provider_error = provider_error;
                                            }
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: terminal_event,
                                                meta: meta.clone(),
                                            }, "[AgentSession] terminal event (Result classify)").await;
                                        }
                                        if clears {
                                            meta.actor = None;
                                        }
                                        emitted_terminal_event = true;
                                        interrupt_escalate_at = None;
                                        claude_text_buf.clear();
                                        claude_thought_buf.clear();
                                        last_thought_persisted_len = 0;
                                        let keep_alive_turn = self.event_bus.in_keep_alive_turn(thread_id);
                                        let commits = !keep_alive_turn && last_terminal_kind.as_ref().is_some_and(result_auto_commits);
                                        if let Some(wt) = worktree_path.as_ref().filter(|_| commits) {
                                            auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Coding agent changes (auto-committed)").await;
                                        }
                                        let (wt_has_changes, wt_requires_restart, changed_files) = if is_resolution_session {
                                            (true, false, None)
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
                                            branch_name = state.branch_name;
                                            (state.has_changes, state.requires_restart, state.changed_files)
                                        };

                                        is_waiting = true;
                                        {
                                            let mut sessions = self.agent_sessions.lock().await;
                                            if let Some(s) = sessions.get_mut(&thread_id) {
                                                crate::engine::agent_session::lifecycle::mark_turn_boundary(s);
                                                s.branch_name = Some(branch_name.clone());
                                                s.idle_notify.notify_waiters();
                                            }
                                        }
                                        inputs.turn_ended();
                                        let mut held_proposal = None;
                                        let ceiling_read = ceiling_delivery.turn_ended();
                                        let ceiling_turn = ceiling_read == Some(true);
                                        let ended_cleanly = turn_ended_cleanly(&last_terminal_kind, is_resolution_session, keep_alive_turn || ceiling_turn);
                                        let ceiling_end = ceiling_turn_end(last_terminal_kind.as_ref(), is_shutdown);
                                        if !is_shutdown && (ceiling_read.is_some() || ceiling_end == CeilingTurnEnd::Brief) {
                                            self.settle_owed_ceiling_handoff(thread_id).await;
                                        }
                                        if ended_cleanly {
                                            self.arm_wait_for_running_background_tasks(thread_id, OwnerTurn::Ended).await;
                                        }
                                        let holds_live_wait = !self.live_waits.for_thread(thread_id).await.is_empty();
                                        let idle_write = idle_change_write(
                                            IdleSession {
                                                is_external_repo,
                                                is_shutdown,
                                                is_resolution_session,
                                                holds_live_wait,
                                                handoff_pending: crate::engine::pending_handoff::handoff_pending(self.pool(), thread_id).await,
                                            },
                                            &last_terminal_kind,
                                        )
                                        .filter(|_| !keep_alive_turn);
                                        let reentry_head = if idle_write == Some(IdleChangeWrite::Complete) {
                                            branch_head_sha(&repo_root, &branch_name).await
                                        } else {
                                            None
                                        };
                                        let proposal_reentry = if emit_idle && idle_write == Some(IdleChangeWrite::Complete) {
                                            reentry_ledger.hold_reentry(
                                                reentry_head.as_deref().unwrap_or_default(),
                                                !crate::engine::thread_hold::may_start_turn(self.pool(), thread_id).await,
                                            )
                                        } else {
                                            crate::engine::change_ops::HoldReentry::default()
                                        };
                                        if idle_write == Some(IdleChangeWrite::HandedOff) {
                                            match crate::engine::change_ops::withhold_handed_off_work(self.pool(), &self.event_bus, thread_id, &repo_root, &branch_name).await {
                                                Ok(true) => self.broadcast_changes_updated().await,
                                                Ok(false) => {}
                                                Err(e) => log_legacy!("[AgentSession] Could not withdraw the handed-off change of thread {}: {}", thread_id, e),
                                            }
                                        } else if let Some(write) = idle_write {
                                            match changed_files.as_deref() {
                                                None => {
                                                    log_legacy!(
                                                        "[AgentSession] Skipping propose/reconcile at idle for thread {}: the diff probe could not be answered",
                                                        thread_id
                                                    );
                                                }
                                                Some([]) => {
                                                    self.reconcile_emptied_pending_change(thread_id, &repo_root, &branch_name).await;
                                                }
                                                Some(_) if !write.may_create() && !self.branch_has_open_change(&branch_name).await => {
                                                    log_legacy!(
                                                        "[AgentSession] Holding the idle proposal for thread {}: it holds a live event wait",
                                                        thread_id
                                                    );
                                                }
                                                Some(changed_files) => {
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
                                                        origin: None,
                                                    }, write.finished(), proposal_reentry).await {
                                                        Ok(ProposeOutcome::Proposed(_)) => {
                                                            proposed_change = true;
                                                        }
                                                        Ok(ProposeOutcome::Held(hold)) => {
                                                            held_proposal = Some(hold);
                                                        }
                                                        Ok(ProposeOutcome::Unfinished | ProposeOutcome::AwaitingLanding(_) | ProposeOutcome::Refused(_)) => {}
                                                        Err(e) => {
                                                            log_legacy!("[AgentSession] Failed to propose change at idle: {}", e);
                                                        }
                                                    }
                                                    self.broadcast_changes_updated().await;
                                                }
                                            }
                                        }

                                        let bg_bash_running = self
                                            .bash_background
                                            .running_for_thread(thread_id)
                                            .await
                                            .iter()
                                            .any(|task| !task.stop_requested);

                                        // INVARIANT: Read before this turn's idle is written: the
                                        // query counts approvals since the latest idle.
                                        // Complete already rules out a Stop, a failure, a
                                        // live event wait and a pending handoff.
                                        let approved_plan_unstarted = idle_write == Some(IdleChangeWrite::Complete)
                                            && emit_idle
                                            && !bg_bash_running
                                            && inputs.owed() == 0
                                            && crate::engine::approved_plan::no_source_change(changed_files.as_deref())
                                            && crate::engine::approved_plan::approved_this_turn_and_idle(self.pool(), thread_id).await;

                                        if emit_idle {
                                            if ended_cleanly {
                                                self.gate_coding_agent_turn_end(thread_id, wt_has_changes, session_start).await;
                                            }
                                            self.emit_coding_agent_idled(
                                                thread_id,
                                                CodingAgentIdleSnapshot {
                                                    has_changes: wt_has_changes,
                                                    is_external_repo,
                                                    requires_restart: wt_requires_restart,
                                                    bg_bash_pending: bg_bash_running,
                                                    worktree_path: worktree_path.as_deref(),
                                                    turn: IdleTurn::of(keep_alive_turn, ceiling_turn),
                                                    turn_end,
                                                },
                                                &meta,
                                                coding_agent,
                                            ).await;
                                            last_emitted_idle = true;
                                            if keep_alive_turn {
                                                self.event_bus.mark_keep_alive_turn(thread_id, false).await;
                                            }
                                        }
                                        withheld_api_error =
                                            self.event_bus.auto_resume_holds().release(thread_id);

                                        let ceiling = if ceiling_end == CeilingTurnEnd::Reenter && emit_idle && !keep_alive_turn && !is_resolution_session {
                                            crate::engine::context_ceiling::ceiling_crossing(self.pool(), thread_id, inputs.owed() > 0).await
                                        } else {
                                            None
                                        };
                                        let mut hold_reentry_sent = false;
                                        let mut reentry_due = false;
                                        let mut ceiling_delivery_failed = false;
                                        if ceiling.is_some() || held_proposal.is_some() || approved_plan_unstarted {
                                            let gate = crate::engine::turn_end::TurnEndState::at_session_idle(
                                                true,
                                                idle_write == Some(IdleChangeWrite::Complete),
                                                crate::engine::turn_end::SessionFacts {
                                                    head: reentry_head,
                                                    held: held_proposal,
                                                    approved_plan_unstarted,
                                                },
                                                ceiling,
                                            );
                                            crate::engine::context_ceiling::recheck_ceiling_approval(&self.event_bus, thread_id, &mut approved_ceiling_reentry).await;
                                            let due = match reentry_ledger.take_due(&gate) {
                                                Some(due) if approved_ceiling_reentry.is_none() && !crate::engine::thread_hold::may_start_turn(self.pool(), thread_id).await => {
                                                    reentry_ledger.forget(due);
                                                    None
                                                }
                                                due => due,
                                            };
                                            reentry_due = due.is_some();
                                            match due {
                                                Some(crate::engine::turn_end::DueReentry::Ceiling(crossing)) => {
                                                    let reentry = match approved_ceiling_reentry.take() {
                                                        Some(input) => Ok(input),
                                                        None => crate::engine::context_ceiling::record_ceiling_reentry(&self.event_bus, thread_id, crossing).await,
                                                    };
                                                    match reentry {
                                                        Ok(reentry) => {
                                                            ceiling_reentry_forwarded = forward_ceiling_input(reentry, &mut inputs, &mut ceiling_delivery, &agent_input_tx);
                                                            if let Some(s) = self.agent_sessions.lock().await.get_mut(&thread_id) {
                                                                s.in_ceiling_reentry = true;
                                                            }
                                                            hold_reentry_sent = ceiling_reentry_forwarded;
                                                            ceiling_delivery_failed = !ceiling_reentry_forwarded;
                                                        }
                                                        Err(e) => {
                                                            log_legacy!("[AgentSession] Ceiling re-entry for thread {} not sent: {}", thread_id, e);
                                                            self.settle_owed_ceiling_handoff(thread_id).await;
                                                        }
                                                    }
                                                }
                                                Some(crate::engine::turn_end::DueReentry::ApprovedPlan) => {
                                                    if let Err(e) = self.send_approved_plan_reentry(thread_id, &msg_tx).await {
                                                        log!("[AgentSession] Approved-plan re-entry for thread {} not sent: {}", thread_id, lucidos_diagnostic_log::private(&e));
                                                    }
                                                }
                                                Some(crate::engine::turn_end::DueReentry::Hold(hold)) => {
                                                    match self.nudge_held_proposal_in_session(thread_id, hold, &msg_tx).await {
                                                        Ok(()) => hold_reentry_sent = true,
                                                        Err(e) => log_legacy!("[AgentSession] Proposal-hold re-entry for thread {} not sent: {}", thread_id, e),
                                                    }
                                                }
                                                None => {}
                                            }
                                        }

                                        if model_swap_reentry_due(ModelSwapIdle {
                                            pending: pending_model_swap.is_some(),
                                            other_reentry: reentry_due,
                                            ended_cleanly,
                                            idle_complete: idle_write == Some(IdleChangeWrite::Complete),
                                            emit_idle,
                                            is_shutdown,
                                            input_owed: inputs.owed() > 0,
                                        }) && crate::engine::thread_hold::may_start_turn(self.pool(), thread_id).await
                                        {
                                            let owed = pending_model_swap
                                                .take()
                                                .filter(|swap| self.cc_model_refusal_untold(&swap.requested.model));
                                            if let Some(swap) = owed {
                                                match self.send_model_swap_reentry(thread_id, &swap, &msg_tx).await {
                                                    Ok(()) => self.record_cc_model_refusal_told(&swap.requested.model),
                                                    Err(e) => log!("[AgentSession] Model-swap re-entry for thread {} not sent: {}", thread_id, lucidos_diagnostic_log::private(&e)),
                                                }
                                            }
                                        }

                                        if let Some(hold) = held_proposal.filter(|hold| proposal_reentry.contains(*hold) && !hold_reentry_sent) {
                                            crate::engine::change_ops::emit_proposal_withheld(
                                                &self.event_bus, thread_id, &branch_name,
                                                changed_files.as_deref().unwrap_or_default(), hold.into(), false,
                                            ).await;
                                        }
                                        if ceiling_delivery_failed { break 'event_loop; }

                                        match idle_action(is_resolution_session, is_shutdown) {
                                            IdleAction::EndSession => {
                                                if self.exit_idle_subprocess_unless_owed(thread_id, msg_rx, inputs.owed(), &agent_cancel, &mut input_gate).await {
                                                    log_legacy!("[AgentSession] Resolution session idle for thread {}: ending loop", thread_id);
                                                    break 'event_loop;
                                                }
                                            }
                                            IdleAction::ExitSubprocess => {
                                                self.exit_idle_subprocess_unless_owed(thread_id, msg_rx, inputs.owed(), &agent_cancel, &mut input_gate).await;
                                            }
                                            IdleAction::Nothing => {}
                                        }
                                    }
                                }
                }

                perm_req = async {
                    match agent_permission_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match perm_req {
                        Some(req) => {
                            let pool = self.pool.clone();
                            let bus = self.event_bus.clone();
                            let pending = self.pending_cc_permission.clone();
                            let workspace_path = self.workspace_path.clone();
                            let session_worktree = worktree_path.clone();
                            let engine = self.try_clone_arc();
                            let tools = tools_in_flight.clone();
                            tokio::spawn(async move {
                                tools.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let mut respond = req.respond;
                                tokio::select! {
                                    outcome = crate::engine::cc_permission::prompt_coding_agent_permission(
                                        &pool,
                                        &bus,
                                        &pending,
                                        &workspace_path,
                                        session_worktree.as_deref(),
                                        engine.as_ref(),
                                        crate::engine::cc_permission::CodingAgentPermissionInput {
                                            thread_id,
                                            tool_use_id: req.id,
                                            tool_name: req.tool_name,
                                            input: req.input,
                                        },
                                    ) => {
                                        let _ = respond.send(outcome.allowed);
                                    }
                                    _ = respond.closed() => {
                                        log_legacy!(
                                            "[AgentSession] agent died with permission card pending for thread {} — abandoning waiter",
                                            thread_id
                                        );
                                    }
                                }
                                release_tool_slot(&tools);
                            });
                        }
                        None => {
                            agent_permission_rx = None;
                        }
                    }
                }

                Some(mut user_input) = msg_rx.recv(), if input_gate.receives() && !ceiling_reentry_forwarded => {
                    if crate::engine::pending_handoff::handoff_pending(self.pool(), thread_id).await {
                        input_gate.handed_off = true;
                        let _sessions = self.agent_sessions.lock().await;
                        requeue_at_head(&msg_tx, msg_rx, user_input);
                        continue 'event_loop;
                    }
                    if crate::engine::thread_stop::is_automatic_input(user_input.kind)
                        && !crate::engine::thread_hold::may_start_turn(self.pool(), thread_id).await {
                        if let Some(id) = user_input.origin_event_id {
                            if let Some(session) = self.agent_sessions.lock().await.get_mut(&thread_id) {
                                session.unforwarded_inputs.remove(&id);
                            }
                        }
                        continue 'event_loop;
                    }
                    if user_input.kind == crate::engine::AgentInputKind::CeilingReentry {
                        approved_ceiling_reentry = Some(user_input);
                        input_gate.ceiling_owed = true;
                        let can_steer = {
                            let mut sessions = self.agent_sessions.lock().await;
                            sessions.get_mut(&thread_id).is_some_and(|s| {
                                s.in_ceiling_reentry = true;
                                is_waiting || s.can_steer
                            })
                        };
                        match crate::engine::context_ceiling::ceiling_steer(
                            &self.event_bus, thread_id, inputs.owed() > 0, can_steer, &mut reentry_ledger, &mut approved_ceiling_reentry,
                        ).await {
                            Ok(Some(reentry)) => {
                                ceiling_reentry_forwarded = forward_ceiling_input(reentry, &mut inputs, &mut ceiling_delivery, &agent_input_tx);
                                if !ceiling_reentry_forwarded { break 'event_loop; }
                            }
                            Ok(None) => {}
                            Err(e) => log_legacy!("[AgentSession] Approved ceiling steer not sent for {thread_id}: {e}"),
                        }
                        continue 'event_loop;
                    }
                    if input_gate.ceiling_owed {
                        held_ceiling_inputs.push(user_input);
                        continue 'event_loop;
                    }
                    if matches!(user_input.kind, crate::engine::AgentInputKind::User | crate::engine::AgentInputKind::Continuation) {
                        match crate::engine::thread_stop::held_inputs(self.pool(), thread_id).await {
                            Ok(held) => {
                                if let Some(session) = self.agent_sessions.lock().await.get_mut(&thread_id) {
                                    crate::engine::thread_stop::queue_held_inputs(session, held, |id| inputs.forwarded_here(id));
                                }
                            }
                            Err(e) => log_legacy!("[AgentSession] Held deliveries of {thread_id} unreadable: {e}"),
                        }
                    }
                    // INVARIANT: The input just left the channel and is about to reach the
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
                            s.in_ceiling_reentry = ceiling_reentry_forwarded;
                            if let Some(id) = user_input.origin_event_id {
                                s.unforwarded_inputs.remove(&id);
                            }
                        }
                    }
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
                    let prompt_sent = if user_input.kind.writes_prompt_sent() {
                        self
                            .event_bus
                            .emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                                    input_event_id: user_input.origin_event_id,
                                    text: user_input.text.clone(),
                                    coding_agent,
                                    origin: None,
                                },
                                meta: meta.clone(),
                            })
                            .await
                            .unwrap_or_else(|e| {
                                log_legacy!("[EventBus] [AgentSession] CodingAgentPromptSent emit failed: {}", e);
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
                        defer_on_steer_failure: false,
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
                    child_reports.publish(inputs.owed_child_reports());
                    if agent_input_tx.send(agent_input).is_err() {
                        log_legacy!("[AgentSession] Failed to forward user input to agent runtime — channel closed");
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
                    child_reports.publish(inputs.owed_child_reports());
                    let action = idle_action(is_resolution_session, self.session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed)));
                    if is_waiting && self.idle_exit_ends_session(thread_id, msg_rx, inputs.owed(), &agent_cancel, action, &mut input_gate).await {
                        break;
                    }
                }

                Some(answered) = pending_withdraws.next() => {
                    finish_withdraw(&self.event_bus, thread_id, &mut inputs, answered).await;
                    child_reports.publish(inputs.owed_child_reports());
                    let action = idle_action(is_resolution_session, self.session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed)));
                    if is_waiting && self.idle_exit_ends_session(thread_id, msg_rx, inputs.owed(), &agent_cancel, action, &mut input_gate).await {
                        break;
                    }
                }

                _ = interrupt.notified() => {
                    let interruption_cause = self.take_session_redirect_cause(thread_id).await;
                    let shutdown_interrupt = self.session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed));
                    if interruption_cause.is_none() && !shutdown_interrupt {
                        if let Some((uuid, anchor)) = ceiling_delivery.pending {
                            if !crate::engine::agent_session::withdraw::withdraw_ceiling_before_stop(
                                &self.event_bus, thread_id, &mut inputs, &agent_withdraw_tx,
                                &mut pending_withdraws, anchor, meta.actor.clone(),
                            ).await {
                                agent_cancel.cancel();
                            }
                            if !inputs.owes(uuid) { ceiling_delivery.pending = None; }
                            if is_waiting {
                                self.settle_owed_ceiling_handoff(thread_id).await;
                                agent_cancel.cancel();
                                break 'event_loop;
                            }
                        }
                    }
                    if !is_waiting {
                        user_hit_stop = true;
                        redirect_cause = interruption_cause;
                        if let Some(cancel_actor) = self.take_session_cancel_actor(thread_id).await {
                            meta.actor = Some(cancel_actor);
                        }
                        log_legacy!("[AgentSession] Sending control_request interrupt to CC process");
                        if agent_control_tx
                            .send(crate::runtime::ControlRequest::Interrupt)
                            .is_err()
                        {
                            log_legacy!("[AgentSession] Failed to forward interrupt: agent control channel closed");
                        }
                        interrupt_escalate_at =
                            Some(tokio::time::Instant::now() + INTERRUPT_ESCALATE_AFTER);
                    }
                }

                _ = stop.notified() => {
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
                        None,
                        &agent_cancel,
                        &mut claude_text_buf,
                        &meta,
                        &external_terminal_emitted,
                        coding_agent,
                    ).await;
                    emitted_terminal_event = true;
                    self.record_usage_flushed_at_stop(
                        &mut events_rx, thread_id, &external_terminal_emitted, &meta, coding_agent, normalized_model.as_deref(), cc_reasoning_effort.as_deref(), &mut billing_attribution, billing_launch_id,
                    ).await;
                    break;
                }

                _ = async move {
                    match escalate_deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    log_legacy!(
                        "[AgentSession] CC did not honor interrupt within {}s — escalating to hard stop for thread {}",
                        INTERRUPT_ESCALATE_AFTER.as_secs(),
                        thread_id
                    );
                    last_terminal_kind = Some(TerminalKind::Canceled(
                        redirect_cause.unwrap_or(crate::engine::thread_events::CancelCause::UserStop),
                    ));
                    withheld_api_error = None;
                    let is_shutdown = self
                        .session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed));
                    self.emit_stop_terminal(
                        "interrupt_escalate",
                        thread_id,
                        is_waiting,
                        is_shutdown,
                        false,
                        redirect_cause,
                        &agent_cancel,
                        &mut claude_text_buf,
                        &meta,
                        &external_terminal_emitted,
                        coding_agent,
                    ).await;
                    emitted_terminal_event = true;
                    self.record_usage_flushed_at_stop(
                        &mut events_rx, thread_id, &external_terminal_emitted, &meta, coding_agent, normalized_model.as_deref(), cc_reasoning_effort.as_deref(), &mut billing_attribution, billing_launch_id,
                    ).await;
                    break;
                }

                _ = chat_cancel.cancelled() => {
                    let is_shutdown = self
                        .session_is_shutting_down(shutting_down.load(std::sync::atomic::Ordering::Relaxed));
                    self.emit_stop_terminal(
                        "chat_cancel",
                        thread_id,
                        is_waiting,
                        is_shutdown,
                        false,
                        None,
                        &agent_cancel,
                        &mut claude_text_buf,
                        &meta,
                        &external_terminal_emitted,
                        coding_agent,
                    ).await;
                    emitted_terminal_event = true;
                    self.record_usage_flushed_at_stop(
                        &mut events_rx, thread_id, &external_terminal_emitted, &meta, coding_agent, normalized_model.as_deref(), cc_reasoning_effort.as_deref(), &mut billing_attribution, billing_launch_id,
                    ).await;
                    break;
                }

                _ = tokio::time::sleep(std::time::Duration::from_secs(WATCHDOG_TICK_INTERVAL_SECS)) => {
                    // INVARIANT: Hung-subprocess watchdog. Any incoming event re-arms the
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
                    if last_ms > 0 {
                        let elapsed_ms = now_ms.saturating_sub(last_ms);
                        if elapsed_ms >= WATCHDOG_DIAG_LOG_THRESHOLD_MS
                            && gate != WatchdogGate::NotStale
                        {
                            log_legacy!(
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
                                let still_running =
                                    crate::engine::thread_liveness::thread_is_running(
                                        self.pool(),
                                        thread_id,
                                    )
                                    .await
                                    .unwrap_or(false);
                                if !still_running {
                                    log_legacy!(
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
                            log_legacy!(
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

        for held in held_ceiling_inputs.into_iter().rev() {
            requeue_at_head(&msg_tx, msg_rx, held);
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

pub(super) struct ResumeSpawnContext<'a> {
    pub worktree_path: Option<&'a Path>,
    pub last_idle_sha: Option<&'a str>,
    pub branch_note: Option<&'a str>,
    pub session_branch: Option<&'a str>,
}

pub(super) const RESUME_NOTE_REFERENT_GUARD: &str =
    "[The lines above are engine status, not the topic. The user's message follows. \
     Resolve its pronouns against the conversation you have already had, not against \
     anything named above.]";

pub(super) enum PromptFor<'a> {
    Turn(&'a str),
    KeepAlive(&'a str),
}

pub(super) struct AgentPrompt {
    pub(super) text: String,
    pub(super) device_block: Option<ContextFingerprint>,
}

impl AgentPrompt {
    fn without_block(text: String) -> Self {
        Self {
            text,
            device_block: None,
        }
    }

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

#[derive(Clone, Copy)]
pub(super) struct HeldBlocks<'a> {
    pub(super) sent: &'a SentDeviceContexts,
    pub(super) conversation: Option<&'a str>,
    pub(super) unread: &'a [ContextFingerprint],
}

impl<'a> HeldBlocks<'a> {
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

fn turn_note(section: &str) -> Option<&str> {
    Some(section.trim()).filter(|note| !note.is_empty())
}

#[derive(Clone, Copy, Default)]
pub(super) struct FollowUpNotes<'a> {
    pub late: Option<&'a str>,
    pub standing: &'a str,
}

pub(super) async fn build_followup_prompt_text(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    origin: Option<Uuid>,
    prompt_sent: Option<Uuid>,
    message: &str,
    notes: FollowUpNotes<'_>,
    session_branch: Option<&str>,
) -> String {
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

pub(super) fn owed_input_event_ids(origin: Option<Uuid>, prompt_sent: Option<Uuid>) -> Vec<Uuid> {
    origin.or(prompt_sent).into_iter().collect()
}

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

fn prepend_resume_notes(thread_id: Uuid, notes: &[&str], message: &str) -> String {
    if notes.is_empty() {
        return message.to_string();
    }
    let block = notes.join("\n");
    log_legacy!(
        "[AgentSession] Injecting resume note for thread {} ({} chars)",
        thread_id,
        block.len()
    );
    format!("{block}\n{RESUME_NOTE_REFERENT_GUARD}\n\n{message}")
}

const HISTORY_MESSAGE_MAX_BYTES: usize = 2000;

const HISTORY_BLOCK_MAX_BYTES: usize = 48 * 1024;

const UNANSWERED_IMAGES_MAX: usize = 8;

#[derive(Debug, Clone, PartialEq)]
struct HistoryLine<'a> {
    role: &'a str,
    channel: Option<&'a str>,
    content: &'a str,
    image_hashes: &'a [String],
    event_id: Option<&'a str>,
    speaker: String,
}

impl<'a> HistoryLine<'a> {
    fn of(
        message: &'a crate::core::store::SessionMessage,
        users: &crate::gateway_users::UserSnapshot,
    ) -> Self {
        Self {
            role: &message.role,
            channel: message.channel.as_deref(),
            content: &message.content,
            image_hashes: &message.user_image_hashes,
            event_id: message.event_id.as_deref(),
            speaker: users.speaker_of_device(message.sender_device_id.as_deref()),
        }
    }

    fn is_reply(&self) -> bool {
        self.role == "assistant"
            && self.channel == Some("claude_code")
            && !self.content.trim().is_empty()
    }
}

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

fn first_unanswered(messages: &[HistoryLine<'_>]) -> usize {
    messages
        .iter()
        .rposition(HistoryLine::is_reply)
        .map_or(0, |at| at + 1)
}

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
                "user" => line.speaker.as_str(),
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
            speaker: "User".into(),
        }
    }

    fn with_images<'a>(line: HistoryLine<'a>, image_hashes: &'a [String]) -> HistoryLine<'a> {
        HistoryLine {
            image_hashes,
            ..line
        }
    }

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

    #[test]
    fn a_team_message_reads_under_its_speaker() {
        let kim = HistoryLine {
            speaker: "Kim".into(),
            ..line("user", None, "try the staging build")
        };
        let block = thread_history_block(&[kim], &[]);
        assert!(block.contains("**Kim:** try the staging build"), "{block}");
    }

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

    #[test]
    fn a_turn_without_a_matching_message_keeps_the_whole_history() {
        let lines = vec![line("user", None, "hello"), line("assistant", None, "hi")];
        assert_eq!(
            before_current_message(lines.clone(), "Your child thread finished."),
            (lines, 2)
        );
    }

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
        drop(tx);
        drain_startup_death(&mut rx)
    }

    #[test]
    fn recovers_driver_result_error() {
        let death = drained(vec![
            AgentEvent::Result {
                text: String::new(),
                duration_ms: 0,
                error: Some("Failed to start Codex: No such file or directory".to_string()),
                provider_error: None,
                turn_end: None,
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
        let death = drained(vec![exited(AgentExitFacts::default())]);
        assert_eq!(death.reason, None);
    }

    #[test]
    fn ignores_empty_error_strings() {
        let death = drained(vec![AgentEvent::Result {
            text: String::new(),
            duration_ms: 0,
            error: Some("   ".to_string()),
            provider_error: None,
            turn_end: None,
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
            crate::engine::start_failure::agent_exit_of(error.as_ref()),
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
            crate::engine::start_failure::agent_exit_of(error.as_ref()),
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

#[cfg(test)]
mod result_text_prose_tests {
    use super::result_text_is_own_prose;

    #[test]
    fn a_slash_command_answer_is_prose() {
        assert!(result_text_is_own_prose("Set model to claude-opus-5", None));
    }

    #[test]
    fn the_turns_failure_reason_is_not_prose() {
        let err = "API Error: Stream idle timeout - no chunks received";
        assert!(!result_text_is_own_prose(err, Some(err)));
        assert!(
            !result_text_is_own_prose(&format!("\n\n{err}\n"), Some(err)),
            "compared trimmed: the buffered copy carries the stream's leading newlines"
        );
    }

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

#[cfg(test)]
mod usage_flushed_at_stop_tests {
    use super::{usage_flushed_at_stop, AgentEvent};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn codex_snapshots_at_stop_keep_order_counts_and_current_billing() {
        use crate::runtime::agent_runtime::CodexUsageSnapshot;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let snapshot = CodexUsageSnapshot {
            session_id: "resumed".into(),
            turn_id: Some("old".into()),
            input_tokens: 2,
            output_tokens: 4,
            cache_read_tokens: 100,
            cache_creation_tokens: 0,
        };
        tx.send(AgentEvent::UsageRestored {
            snapshot: snapshot.clone(),
        })
        .unwrap();
        tx.send(usage(100)).unwrap();
        tx.send(AgentEvent::UsageUnscoped { snapshot }).unwrap();
        let mut billing = super::BillingAttribution::codex_account(
            Some("openai"),
            &serde_json::json!({
                "requiresOpenaiAuth": true, "account": {"type": "apiKey"}
            }),
        )
        .codex_endpoint_checked(true);
        let original = billing.clone();
        drop(tx);
        let flushed =
            usage_flushed_at_stop(&mut rx, std::time::Duration::from_secs(1), &mut billing).await;
        assert_eq!(flushed.len(), 3);
        assert!(flushed[0].1.restored.is_some());
        assert_eq!(flushed[1].1.billing, original);
        assert_eq!(flushed[2].1.billing.provider_id(), "unknown");
        assert_eq!(billing, original);
        for (_, usage) in flushed {
            assert_eq!(
                (
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_tokens
                ),
                (2, 4, 100)
            );
        }
    }

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

    #[tokio::test]
    async fn queued_billing_evidence_is_kept_for_a_cut_off_call() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gateway = super::BillingAttribution::claude_initialize(&serde_json::json!({
            "account": {"apiProvider": "gateway"}
        }));
        tx.send(AgentEvent::BillingObserved {
            attribution: gateway.clone(),
        })
        .unwrap();
        tx.send(usage(100)).unwrap();
        tx.send(AgentEvent::BillingObserved {
            attribution: super::BillingAttribution::default(),
        })
        .unwrap();
        tx.send(usage(200)).unwrap();
        tx.send(AgentEvent::OutputEnded).unwrap();
        let mut billing = super::BillingAttribution::default();
        let flushed =
            usage_flushed_at_stop(&mut rx, std::time::Duration::from_secs(1), &mut billing).await;
        assert_eq!(flushed.len(), 2);
        assert_eq!(flushed[0].1.billing, gateway);
        assert_eq!(flushed[0].1.cache_read_tokens, 100);
        assert_eq!(flushed[1].1.billing, super::BillingAttribution::default());
        assert_eq!(flushed[1].1.cache_read_tokens, 200);
        assert_eq!(billing, super::BillingAttribution::default());
    }

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

        let flushed = usage_flushed_at_stop(
            &mut rx,
            std::time::Duration::from_secs(5),
            &mut super::BillingAttribution::default(),
        )
        .await;

        let cache_reads: Vec<u32> = flushed.iter().map(|(_, u)| u.cache_read_tokens).collect();
        assert_eq!(cache_reads, vec![100, 200]);
        assert_eq!(flushed[0].0.as_deref(), Some("claude-sonnet-5"));
        assert!(
            matches!(rx.try_recv(), Ok(AgentEvent::Usage { .. })),
            "the read stops at OutputEnded"
        );
    }

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

        let flushed = usage_flushed_at_stop(
            &mut rx,
            std::time::Duration::from_secs(5),
            &mut super::BillingAttribution::default(),
        )
        .await;

        assert_eq!(
            flushed[0].1.parent_tool_use_id.as_deref(),
            Some("toolu_agent")
        );
    }

    #[tokio::test]
    async fn a_closed_channel_ends_the_read() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(usage(7)).expect("send");
        drop(tx);

        let flushed = usage_flushed_at_stop(
            &mut rx,
            std::time::Duration::from_secs(5),
            &mut super::BillingAttribution::default(),
        )
        .await;

        assert_eq!(flushed.len(), 1);
    }

    #[tokio::test]
    async fn the_read_stops_at_exited_and_keeps_the_usage_before_it() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        for ev in [
            usage(100),
            AgentEvent::Exited {
                exit: super::AgentExitFacts::default(),
            },
            usage(300),
        ] {
            tx.send(ev).expect("send");
        }

        let flushed = usage_flushed_at_stop(
            &mut rx,
            std::time::Duration::from_secs(1),
            &mut super::BillingAttribution::default(),
        )
        .await;

        let cache_reads: Vec<u32> = flushed.iter().map(|(_, u)| u.cache_read_tokens).collect();
        assert_eq!(cache_reads, vec![100]);
        assert!(
            matches!(rx.try_recv(), Ok(AgentEvent::Usage { .. })),
            "the read stops at Exited"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_driver_cannot_hold_the_stop() {
        let (_tx, mut rx) = mpsc::unbounded_channel();

        let flushed = usage_flushed_at_stop(
            &mut rx,
            std::time::Duration::from_secs(2),
            &mut super::BillingAttribution::default(),
        )
        .await;

        assert!(flushed.is_empty());
    }
}

#[cfg(test)]
mod call_models_tests {
    use super::CallModels;
    use crate::llm::served_model::served_as_sent;

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

    #[test]
    fn an_alias_session_with_1m_keeps_the_marker_on_the_full_id() {
        let models = CallModels::of(Some("opus[1m]"), Some("claude-opus-4-6"));

        assert_eq!(models.model, "claude-opus-4-6[1m]");
        assert!(served_as_sent(&models.model, "claude-opus-4-6"));
    }

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

    #[test]
    fn a_main_session_call_is_served_as_sent() {
        let models = CallModels::of(Some("claude-opus-5-5[1m]"), Some("claude-opus-5-5"));

        assert_eq!(models.model, "claude-opus-5-5[1m]");
        assert_eq!(models.served_model.as_deref(), Some("claude-opus-5-5"));
        assert!(served_as_sent(&models.model, "claude-opus-5-5"));
    }

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

#[cfg(test)]
mod coding_agent_capture_tests {
    use super::{
        coding_agent_capture, effort_in_force, CallModels, ReportedUsage, RunAgentSessionExt,
    };
    use crate::engine::agent_session::runtime_helpers::RuntimeHelpersExt;
    use crate::runtime::CodingAgent;

    #[tokio::test]
    async fn context_ceiling_stop_and_failure_record_a_brief_and_run_no_turn() {
        use crate::engine::agent_session::lifecycle::{
            ceiling_turn_end, CeilingTurnEnd, TerminalKind,
        };
        use crate::engine::context_ceiling::{record_ceiling_reached, CeilingCrossing};
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{CancelCause, EventMeta};
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, start_cc_session, teardown_test_db,
        };
        use uuid::Uuid;
        let name = concat!(
            module_path!(),
            "::context_ceiling_stop_and_failure_record_a_brief_and_run_no_turn"
        );
        if !in_engine_child(name.split_once("::").unwrap().1).await {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        for terminal in [
            TerminalKind::Canceled(CancelCause::UserStop),
            TerminalKind::Failed {
                error: "failure".into(),
            },
        ] {
            let thread = Uuid::new_v4();
            start_cc_session(&engine.event_bus, thread, "test/context-ceiling", None).await;
            record_ceiling_reached(
                &engine.event_bus,
                thread,
                CeilingCrossing {
                    context_tokens: 301_000,
                    ceiling: 300_000,
                    window: Some(1_000_000),
                },
            )
            .await
            .unwrap();
            assert_eq!(
                ceiling_turn_end(Some(&terminal), false),
                CeilingTurnEnd::Brief
            );
            engine
                .event_bus
                .emit(BusEvent::Thread {
                    thread_id: thread,
                    event: crate::engine::LucidosEngine::make_terminal_event(
                        terminal,
                        String::new(),
                        None,
                        None,
                    ),
                    meta: EventMeta::NONE,
                })
                .await
                .unwrap();
            engine.settle_owed_ceiling_handoff(thread).await;
            let handoffs: Vec<serde_json::Value> = sqlx::query_scalar(
                "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'SessionHandedOff'"
            ).bind(thread).fetch_all(&pool).await.unwrap();
            assert_eq!(handoffs.len(), 1);
            assert_eq!(handoffs[0]["opening"], "brief");
            assert_eq!(handoffs[0]["cause"], "context-ceiling");
            assert_eq!(handoffs[0]["starts"], "on-next-input");
            let prompts: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'PromptInjected'"
            ).bind(thread).fetch_one(&pool).await.unwrap();
            assert_eq!(prompts, 0);
        }
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn context_ceiling_capture_recording_ignores_restored_and_child_usage() {
        use crate::engine::context_ceiling::past_ceiling;
        use crate::engine::thread_events::EventMeta;
        use crate::runtime::agent_runtime::CodexUsageSnapshot;
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, start_cc_session, teardown_test_db,
        };
        use std::sync::atomic::AtomicBool;
        use uuid::Uuid;
        let name = concat!(
            module_path!(),
            "::context_ceiling_capture_recording_ignores_restored_and_child_usage"
        );
        if !in_engine_child(name.split_once("::").unwrap().1).await {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let thread = Uuid::new_v4();
        start_cc_session(&engine.event_bus, thread, "test/context-ceiling", None).await;
        let token = AtomicBool::new(false);
        let launch = Uuid::new_v4();
        for (tokens, restored, child) in [
            (60_000, false, false),
            (400_000, true, false),
            (500_000, false, true),
            (301_000, false, false),
            (654_000, false, false),
        ] {
            let mut usage = ReportedUsage::unscoped(CodexUsageSnapshot {
                session_id: "test-session".into(),
                turn_id: Some("turn".into()),
                input_tokens: tokens,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            });
            if restored {
                usage.restored = Some(CodexUsageSnapshot {
                    session_id: "test-session".into(),
                    turn_id: Some("old-turn".into()),
                    input_tokens: tokens,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                });
            }
            usage.parent_tool_use_id = child.then(|| "child-tool".into());
            engine
                .record_coding_agent_usage(
                    thread,
                    &token,
                    &EventMeta::NONE,
                    CodingAgent::ClaudeCode,
                    CallModels::of(Some("claude-opus-5-5"), None),
                    usage,
                    None,
                    launch,
                )
                .await;
            let crossing = past_ceiling(&pool, thread).await.unwrap();
            if tokens < 300_000 || restored || child {
                assert_eq!(crossing, None);
            } else {
                assert_eq!(crossing.unwrap().context_tokens, 301_000);
            }
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ContextCeilingReached'"
        ).bind(thread).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn codex_resume_recording_preserves_snapshots_without_pinning_new_billing() {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};
        use crate::runtime::agent_runtime::CodexUsageSnapshot;
        use crate::runtime::billing_provider::{
            BillingAttribution, BillingEvidenceSource, BillingProvider,
        };
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, start_cc_session, teardown_test_db,
        };
        use std::sync::atomic::AtomicBool;
        use uuid::Uuid;
        let name = concat!(
            module_path!(),
            "::codex_resume_recording_preserves_snapshots_without_pinning_new_billing"
        );
        let name = name.split_once("::").expect("engine crate prefix").1;
        if !in_engine_child(name).await {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let thread = Uuid::new_v4();
        let launch = Uuid::new_v4();
        let live_billing = BillingAttribution::Known {
            provider: BillingProvider::ChatgptPlan,
            source: BillingEvidenceSource::CodexAccount,
        };
        let settings = || ThreadEvent::CodingAgentSettingsChanged {
            model: None,
            reasoning_effort: None,
            coding_agent: CodingAgent::Codex,
            cc_session_id: Some("codex-session".into()),
            claude_config_dir: None,
            claude_config_dir_explicit: None,
        };
        start_cc_session(&engine.event_bus, thread, "test/resume-provider", None).await;
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: thread,
                event: settings(),
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        let mut original = reported();
        original.billing = BillingAttribution::Known {
            provider: BillingProvider::Openai,
            source: BillingEvidenceSource::CodexAccount,
        };
        let original = coding_agent_capture(
            CodingAgent::Codex,
            CallModels::of(Some("test-model"), None),
            200_000,
            original,
            None,
        );
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: thread,
                event: original,
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        start_cc_session(&engine.event_bus, thread, "test/resume-provider", None).await;
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: thread,
                event: settings(),
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        engine
            .observe_coding_agent_billing(
                thread,
                CodingAgent::Codex,
                launch,
                &live_billing,
                &EventMeta::NONE,
            )
            .await;
        let snapshot = CodexUsageSnapshot {
            session_id: "codex-session".into(),
            turn_id: Some("old-turn".into()),
            input_tokens: 2,
            output_tokens: 4,
            cache_read_tokens: 8,
            cache_creation_tokens: 0,
        };
        let token = AtomicBool::new(false);
        let unscoped = snapshot.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut billing = live_billing.clone();
        tokio::join!(
            engine.record_usage_flushed_at_stop(
                &mut rx,
                thread,
                &token,
                &EventMeta::NONE,
                CodingAgent::Codex,
                Some("test-model"),
                None,
                &mut billing,
                launch,
            ),
            async move {
                tokio::task::yield_now().await;
                use crate::runtime::agent_runtime::AgentEvent;
                for _ in 0..2 {
                    tx.send(AgentEvent::UsageRestored {
                        snapshot: snapshot.clone(),
                    })
                    .unwrap();
                }
                tx.send(AgentEvent::Usage {
                    model: None,
                    input_tokens: 2,
                    output_tokens: 4,
                    cache_read_tokens: 8,
                    cache_creation_tokens: 0,
                    cache_creation_1h_tokens: None,
                    parent_tool_use_id: None,
                    api_call_id: None,
                    duration_ms: None,
                    reasoning_effort: None,
                })
                .unwrap();
            }
        );
        engine
            .record_coding_agent_usage(
                thread,
                &token,
                &EventMeta::NONE,
                CodingAgent::Codex,
                CallModels::of(Some("test-model"), None),
                ReportedUsage::unscoped(unscoped),
                None,
                launch,
            )
            .await;
        let events = engine
            .event_store
            .get_thread_events(&thread.to_string())
            .await
            .unwrap();
        let captures: Vec<_> = events
            .iter()
            .filter(|row| row.event_type == "ContextCaptured")
            .map(|row| &row.payload)
            .collect();
        assert_eq!(captures.len(), 5);
        assert_eq!(
            captures
                .iter()
                .map(|p| p["provider"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["openai", "openai", "openai", "chatgpt-plan", "unknown"]
        );
        for capture in &captures[1..] {
            assert_eq!(capture["usage"], captures[0]["usage"]);
        }
        let observations: Vec<_> = events
            .iter()
            .filter(|row| row.event_type == "CodingAgentBillingObserved")
            .collect();
        assert_eq!(
            observations.len(),
            1,
            "snapshot attribution must not repin billing"
        );
        assert_eq!(
            observations[0].payload["attribution"]["provider"],
            "chatgpt-plan"
        );
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_coding_agent_capture_takes_the_backends_own_context_window() {
        use crate::engine::thread_events::EventMeta;
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, start_cc_session, teardown_test_db,
        };
        use std::sync::atomic::AtomicBool;
        use uuid::Uuid;
        let name = concat!(
            module_path!(),
            "::a_coding_agent_capture_takes_the_backends_own_context_window"
        );
        if !in_engine_child(name.split_once("::").unwrap().1).await {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let thread = Uuid::new_v4();
        start_cc_session(&engine.event_bus, thread, "test/backend-window", None).await;
        let token = AtomicBool::new(false);
        engine
            .record_coding_agent_usage(
                thread,
                &token,
                &EventMeta::NONE,
                CodingAgent::ClaudeCode,
                CallModels::of(Some("claude-sonnet-5"), Some("claude-sonnet-5")),
                reported(),
                None,
                Uuid::new_v4(),
            )
            .await;
        let events = engine
            .event_store
            .get_thread_events(&thread.to_string())
            .await
            .unwrap();
        let capture = events
            .iter()
            .find(|row| row.event_type == "ContextCaptured")
            .expect("the call was captured");
        assert_eq!(
            capture.payload["context_window"], 1_000_000,
            "a bare Sonnet 5 id runs 1M in Claude Code, where the registry answers 200k"
        );
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    fn reported() -> ReportedUsage {
        ReportedUsage {
            billing: super::BillingAttribution::default(),
            restored: None,
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

    #[tokio::test]
    async fn resumed_keep_alive_captures_preserve_process_billing() {
        use crate::engine::event_bus::EventBus;
        use crate::runtime::billing_provider::BillingAttribution;
        use crate::test_support::{
            seed_thread_event, setup_test_db, start_cc_session, teardown_test_db,
        };
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = uuid::Uuid::new_v4();
        start_cc_session(&bus, thread, "test/keep-alive-billing", None).await;
        let _held = bus.begin_keep_alive_turn(thread).await;
        for billing in [
            BillingAttribution::claude_initialize(&serde_json::json!({
                "account": {"apiProvider": "gateway"}
            })),
            BillingAttribution::default(),
        ] {
            let expected = billing.provider_id();
            let mut usage = reported();
            usage.billing = billing;
            let capture = coding_agent_capture(
                CodingAgent::ClaudeCode,
                CallModels::of(Some("test-model"), None),
                200_000,
                usage,
                None,
            );
            let id = seed_thread_event(&bus, thread, capture).await;
            let payload: serde_json::Value =
                sqlx::query_scalar("SELECT payload FROM events WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(payload["provider"], expected);
            assert_eq!(payload["model"], "test-model");
        }
        bus.mark_keep_alive_turn(thread, false).await;
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[test]
    fn a_coding_agent_capture_never_silently_omits_provider() {
        let payload = captured_payload(None);
        assert!(payload["provider"].is_string(), "{payload}");
    }

    #[test]
    fn old_capture_without_provider_still_replays_as_unattributed() {
        let mut payload = captured_payload(None);
        payload.as_object_mut().unwrap().remove("provider");
        let event: crate::engine::thread_events::ThreadEvent =
            serde_json::from_value(payload).unwrap();
        let replay = serde_json::to_value(event).unwrap();
        assert!(replay.get("provider").is_none());
    }

    #[test]
    fn provider_is_independent_of_producer_and_model() {
        for agent in CodingAgent::ALL {
            let mut usage = reported();
            usage.billing = super::BillingAttribution::claude_initialize(&serde_json::json!({
                "account": {"apiProvider": "gateway"}
            }));
            let event = coding_agent_capture(
                agent,
                CallModels::of(Some("test-model"), None),
                200_000,
                usage,
                None,
            );
            let payload = serde_json::to_value(event).unwrap();
            assert_eq!(payload["provider"], "claude-gateway");
            assert_eq!(payload["model"], "test-model");
            assert_eq!(payload["usage"]["input_tokens"], 10);
        }
    }

    #[test]
    fn a_claude_code_capture_carries_the_effort_the_session_ran_at() {
        let payload = captured_payload(Some("high".into()));

        assert_eq!(payload["producer"], "claude_code");
        assert_eq!(payload["reasoning_effort"], "high");
    }

    #[test]
    fn an_unknown_effort_is_omitted() {
        let payload = captured_payload(None);

        assert!(payload.get("reasoning_effort").is_none(), "{payload}");
    }

    #[test]
    fn claude_code_runs_at_its_spawn_effort() {
        assert_eq!(
            effort_in_force(CodingAgent::ClaudeCode, Some("medium"), Some("max".into())),
            Some("medium".into())
        );
    }

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

#[cfg(test)]
mod context_ceiling_delivery_tests {
    use super::*;

    fn forward() -> (CeilingDelivery, InputLedger, AgentInput) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut delivery = CeilingDelivery::default();
        let mut ledger = InputLedger::new();
        assert!(forward_ceiling_input(
            AgentUserInput {
                text: "Hand off".into(),
                images: None,
                origin_event_id: Some(Uuid::new_v4()),
                kind: crate::engine::AgentInputKind::CeilingReentry,
                completion: None,
            },
            &mut ledger,
            &mut delivery,
            &tx
        ));
        (delivery, ledger, rx.try_recv().unwrap())
    }

    #[test]
    fn context_ceiling_merged_read_preserves_user_turn_decisions() {
        let (mut delivery, mut ledger, input) = forward();
        assert!(input.defer_on_steer_failure);
        assert_eq!(
            delivery.turn_ended(),
            None,
            "a Result before the read settles nothing"
        );
        let read = AgentEvent::InputRead { input: input.uuid };
        assert!(matches!(ledger.observe(&read), Settled::Read(_)));
        delivery.observe(&read, false);
        assert_eq!(delivery.turn_ended(), Some(false));
        assert_eq!(delivery.turn_ended(), None);
    }

    #[test]
    fn context_ceiling_read_after_result_is_an_engine_turn() {
        let (mut delivery, mut ledger, input) = forward();
        ledger.turn_ended();
        assert_eq!(delivery.turn_ended(), None);
        assert!(ledger.owes(input.uuid));
        let read = AgentEvent::InputRead { input: input.uuid };
        ledger.observe(&read);
        delivery.observe(&read, true);
        assert_eq!(delivery.turn_ended(), Some(true));
    }

    #[test]
    fn context_ceiling_crossing_holds_followups_before_any_steer() {
        let gate = InputGate {
            ceiling_owed: true,
            ..InputGate::default()
        };
        assert!(!gate.forwards());
    }

    #[tokio::test]
    async fn context_ceiling_stop_withdraws_unread_steer_before_interrupt() {
        use crate::engine::agent_session::withdraw::withdraw_ceiling_before_stop;
        use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool);
        let thread = seed_coding_thread(&bus, None).await;
        let (delivery, mut ledger, input) = forward();
        let anchor = delivery.pending.unwrap().1;
        let (driver, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::runtime::WithdrawRequest>();
        let responder = async {
            let withdraw = rx.recv().await.unwrap();
            assert_eq!(withdraw.input_uuid, input.uuid);
            withdraw
                .reply
                .send(crate::runtime::InputWithdrawal::Withdrawn)
                .unwrap();
        };
        let mut pending = PendingWithdraws::new();
        let (settled, ()) = tokio::join!(
            withdraw_ceiling_before_stop(
                &bus,
                thread,
                &mut ledger,
                &driver,
                &mut pending,
                anchor,
                None
            ),
            responder,
        );
        assert!(settled);
        assert_eq!(ledger.owed(), 0);
        assert_eq!(
            ledger.observe(&AgentEvent::InputRead { input: input.uuid }),
            Settled::Nothing
        );
        teardown_test_db(&db).await;
    }
}

#[cfg(test)]
mod loop_rule_tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};

    #[test]
    fn an_unpaired_tool_result_leaves_the_counter_at_zero() {
        let counter = AtomicI32::new(0);
        release_tool_slot(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        release_tool_slot(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_paired_release_counts_down_one_slot_at_a_time() {
        let counter = AtomicI32::new(2);
        release_tool_slot(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
        release_tool_slot(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        release_tool_slot(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_handoff_or_a_provider_block_leaves_input_in_the_channel() {
        let handed_off = InputGate {
            handed_off: true,
            ..InputGate::default()
        };
        let blocked = InputGate {
            provider_blocked: true,
            ..InputGate::default()
        };
        for gate in [handed_off, blocked] {
            assert!(!gate.receives());
            assert!(!gate.forwards());
        }
        assert!(InputGate::default().receives());
        assert!(InputGate::default().forwards());
    }

    #[test]
    fn a_ceiling_debt_still_receives_input_for_the_successor_but_forwards_none() {
        let gate = InputGate {
            ceiling_owed: true,
            ..InputGate::default()
        };
        assert!(gate.receives());
        assert!(!gate.forwards());
    }
}
