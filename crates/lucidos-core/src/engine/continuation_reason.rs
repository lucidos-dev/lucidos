//! Coding-agent continuation markers and retry decisions.

pub const TURN_RUNNING_REASON: &str = "turn-running";

pub const AUTO_HARDEN_MESSAGE: &str = "Run /harden now.";

/// Tag a Claude Code session that crashed mid-turn for manual Continue. The
/// frontend shows the affordance on `CodingAgentIdled`. The user's click
/// produces the next `ContinuationRequested`.
pub const ENGINE_RESTART_INTERRUPT_REASON: &str = "engine_restart_interrupt";

/// Tag stamped on `ContinuationRequested.reason` when the user clicks the
/// "click to continue" affordance after a mid-turn interrupt. The continue
/// endpoint emits with this reason, and the spawn dispatcher starts the next
/// CC turn.
pub const USER_CLICKED_CONTINUE_REASON: &str = "user_clicked_continue";

/// Tag stamped on `ContinuationRequested.reason` when the user answers an
/// `AskUserQuestion` after the coding-agent subprocess has been torn down at
/// idle. `notify()` is a no-op in that window, so this signal makes the spawn
/// dispatcher boot a fresh `--resume` subprocess.
///
/// `continue_input_for_reason` carries the answer itself. When Claude Code
/// exits, it records a rejection against its pending tool call. A resumed
/// session has no pending hook that could recover the answer.
pub const ANSWERED_AFTER_IDLE_REASON: &str = "answered_after_idle";

/// Reason for a continuation after the watchdog kills a silent Claude Code
/// process. It requests automatic `--resume` and opens a new response boundary.
pub const AUTO_RECOVERY_AFTER_HANG_REASON: &str = "auto_recovery_after_hang";

/// Tag stamped on `ContinuationRequested.reason` when the backend ended a turn
/// on a TRANSIENT upstream API failure (its own `API Error: …` message, e.g. a
/// connection closed mid-response) and the engine resumes the session instead of
/// leaving the thread dead behind a red dot.
///
/// The watchdog already handles a network failure that leaves the backend
/// silent. This reason covers a failure that the backend reports before
/// exiting.
///
/// Unlike the watchdog reasons this one is BOUNDED
/// (`MAX_API_ERROR_AUTO_RESUMES`), because the trigger is a failure the backend
/// might reproduce immediately: a persistently broken upstream must surface,
/// not loop. See `auto_resume_after_api_error`.
pub const AUTO_RESUME_AFTER_API_ERROR_REASON: &str = "auto_resume_after_api_error";

/// Tag stamped on `ContinuationRequested.reason` when the owner allowed a
/// remote start's coding-agent run (`spawn_gate::remote_start`). The run gets
/// the prompt it never had.
pub const OWNER_ALLOWED_START_REASON: &str = "owner_allowed_start";

/// Tag stamped on `ContinuationRequested.reason` when recovery auto-resumes an
/// in-flight coding-agent thread after a **user-initiated** *Switch to new
/// version* (detected by a device-attributed teardown `ResponseAborted`). A crash
/// leaves no such boundary → the thread gets the manual "Continue" affordance
/// instead, so work that may have crashed the engine can't loop. Opens a
/// "Resumed" boundary exchange like the other automatic-resume reasons.
pub const AUTO_RESUME_AFTER_SWITCH_REASON: &str = "auto_resume_after_switch";

/// Tag stamped on `ContinuationRequested.reason` when the user presses
/// **Harden** on a thread whose work was held for `hardening_missing`. The
/// resume carries `AUTO_HARDEN_MESSAGE` (`continue_input_for_reason`). The
/// user pressed a button that starts a new turn, so it opens a resume
/// exchange, as Continue does.
pub const HARDEN_REQUESTED_REASON: &str = "harden_requested";

/// Tag an older `ContinuationRequested.reason` carries: the owner let a
/// paused thread's plan nudge run from a step card. Nothing emits it since
/// Hold replaced the retired automation pause. Recovery still resumes stored ones.
pub const PLAN_REQUESTED_REASON: &str = "plan_requested";

/// Tag an older `ContinuationRequested.reason` carries: the owner let a
/// paused session past its context ceiling hand off, from a step card.
pub const HANDOFF_REQUESTED_REASON: &str = "handoff_requested";

/// Tag an older `ContinuationRequested.reason` carries: the owner let a
/// paused thread build the plan they approved, from a step card.
pub const BUILD_REQUESTED_REASON: &str = "build_requested";

/// Tag stamped on `ContinuationRequested.reason` when a hold or a card held
/// child reports, and its end asks the agent to read them. The turn-gap note
/// carries the reports themselves.
pub const REPORTS_REQUESTED_REASON: &str = "reports_requested";

/// The resume message of [`REPORTS_REQUESTED_REASON`].
pub const REPORTS_REQUESTED_MESSAGE: &str = "Reports arrived while this thread was on hold or \
     waited on a card. That has ended, so read them, then carry on.";

/// Tag an older `ContinuationRequested.reason` carries: a Claude Code effort
/// pick restarted the session mid-turn and resumed it. Nothing emits it since
/// the rung locks for a turn (ADR 0542). The timeline still renders old ones.
pub const EFFORT_RESTART_REASON: &str = "effort_restart";

/// Decide whether a continuation emits `ContinuationStarted` and opens a new
/// response exchange.
///
/// Manual Continue after an interruption and automatic retries after a watchdog
/// or API failure open a new exchange. Their previous response ended.
///
/// An answer after idle continues under its existing `UserQuestionAsked`
/// exchange. Unknown reasons must opt in before they can open a new boundary.
pub fn continue_should_open_resume_exchange(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some(USER_CLICKED_CONTINUE_REASON)
            | Some(HARDEN_REQUESTED_REASON)
            | Some(PLAN_REQUESTED_REASON)
            | Some(BUILD_REQUESTED_REASON)
            | Some(HANDOFF_REQUESTED_REASON)
            | Some(REPORTS_REQUESTED_REASON)
            | Some(AUTO_RECOVERY_AFTER_HANG_REASON)
            | Some(AUTO_RESUME_AFTER_SWITCH_REASON)
            | Some(AUTO_RESUME_AFTER_API_ERROR_REASON)
    )
}

/// What the spawn consumer must do after a `SpawnRequest::Continue`'s
/// `run_direct_agent` returns. See [`continue_recovery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinueRecovery {
    /// The turn ran; the run loop emitted its own terminal. Nothing to do.
    Nothing,
    /// Stale resume on the FIRST attempt — re-run once with a fresh session
    /// (no resume sid) and the conversation reconstructed into the prompt.
    RetryFresh,
    /// Errored with no retry left. Settle the projection so the thread cannot
    /// sit at `running` with no live subprocess.
    Settle,
}

/// Choose the spawn consumer's next action from the continuation result and
/// retry state.
///
/// A stale resume returns without a terminal event so the projection stays
/// running during the retry. It also drops the session entry. Retry once with a
/// fresh session, then settle if another error occurs.
///
/// If another run owns the turn, that run emits its own terminal. Leave it
/// running. Settle other errors after checking that the projection still reads
/// `running`.
pub fn continue_recovery(error: Option<&str>, retried: bool) -> ContinueRecovery {
    match error {
        None => ContinueRecovery::Nothing,
        Some(e) if e == AGENT_ALREADY_RUNNING_ERROR => ContinueRecovery::Nothing,
        Some(e) if !retried && e == STALE_RESUME_ERROR => ContinueRecovery::RetryFresh,
        Some(_) => ContinueRecovery::Settle,
    }
}

/// Sentinel error message returned when a CC resume attempt produces an empty
/// Result immediately — the session was stale (e.g. expired after idle timeout).
/// Callers should retry with a fresh session.
pub const STALE_RESUME_ERROR: &str = "CC_STALE_RESUME";

/// Error a spawn reports when its claim found a **live** session on the
/// thread (`claim_thread_for_spawn` returned `None`). The caller owns nothing.
///
/// Shared error for a rejected send when another live session owns the turn.
/// Callers with a settlement backstop must leave that session running. See
/// `agent_recovery::continue_recovery`.
pub const AGENT_ALREADY_RUNNING_ERROR: &str =
    "A coding agent is already running for this thread. Cancel it first or wait for it to finish.";
