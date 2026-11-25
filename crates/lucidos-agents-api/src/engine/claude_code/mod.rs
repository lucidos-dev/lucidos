use super::{thread_liveness::thread_is_running, LucidosEngine};
use crate::core::grants::GrantFile;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::tool_grants;
use crate::engine::thread_events::MessageOrigin;
use crate::engine::types::CcCommandsInfo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) mod control;
pub(crate) mod merge_session;
mod model_discovery;
pub use model_discovery::start_cc_model_discovery;
pub(crate) use model_discovery::CcModelDiscoveryExt;
pub(crate) mod spawn;

/// Look up a repo's cached CC commands by its on-disk path.
///
/// Returns the cached entry for `repo_path`, or empty defaults on a miss. The
/// compose menu only shows skills for the selected repository.
pub(crate) fn lookup_repo_commands_in_cache(
    cache: &HashMap<String, CcCommandsInfo>,
    repo_path: &str,
) -> CcCommandsInfo {
    cache.get(repo_path).cloned().unwrap_or_default()
}

pub(crate) const ENGINE_ALLOWED_TOOLS: &[&str] = &[
    "Bash(lucidos request-read:*)",
    "Bash(lucidos expect-wake:*)",
];

/// Resolve the comma-separated tool allowlist for `claude --allowedTools`:
/// [`ENGINE_ALLOWED_TOOLS`], then the workspace's standing coding-agent grants.
pub(crate) async fn cc_allowed_tools(bus: &EventBus) -> String {
    allowed_tools_flag(
        tool_grants::granted_lines(bus, GrantFile::CodingAgentTools, None)
            .await
            .standing,
    )
}

/// [`ENGINE_ALLOWED_TOOLS`], then each of `grants` Claude Code may match
/// itself, comma-separated.
pub(crate) fn allowed_tools_flag(grants: Vec<String>) -> String {
    let grants = grants.into_iter().filter(|pattern| {
        let engine_only = only_the_engine_may_match(pattern);
        if engine_only {
            log_legacy!(
                "[ClaudeCode] Left {} out of --allowedTools: Claude Code would match it unchecked",
                pattern
            );
        }
        !engine_only
    });
    ENGINE_ALLOWED_TOOLS
        .iter()
        .map(|tool| tool.to_string())
        .chain(grants)
        .collect::<Vec<_>>()
        .join(",")
}

fn only_the_engine_may_match(pattern: &str) -> bool {
    pattern == "Bash" || pattern.starts_with("Bash(")
}

/// A canceled Claude Code subprocess returns `Ok(_)`, so the inner future succeeding
/// is not enough to prove `/harden` ran. The marker is the only signal that
/// survives a kill — it's only written by the skill's final phase.
pub(crate) fn hardening_succeeded(cancel_token_cancelled: bool, marker_present: bool) -> bool {
    !cancel_token_cancelled && marker_present
}

pub(crate) fn change_applied_concurrently(
    change_status: Option<crate::core::changes::ChangeStatus>,
) -> bool {
    change_status == Some(crate::core::changes::ChangeStatus::Applied)
}

pub(crate) use crate::engine::continuation_reason::STALE_RESUME_ERROR;

pub(crate) use crate::engine::types::SpawnAgentThreadParams;

/// Emit `ResponseFailed` for a background task that errored, but only if the
/// projection still shows the thread as `running`. Prevents double-terminal
/// in the common case where Stop/Discard already settled the thread; see
/// `thread_is_running` for the race caveat.
pub(crate) async fn emit_background_task_failure(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    error: impl std::fmt::Display,
    label: &str,
) {
    // A read that failed is not "the thread already settled". The conservative
    // side avoids a double terminal. It also drops the very failure this
    // function exists to report, so the reason has to reach the log.
    let still_running = match thread_is_running(engine.pool(), thread_id).await {
        Ok(running) => running,
        Err(e) => {
            log_legacy!(
                "[ClaudeCode] Cannot tell whether thread {} is still running ({}), so {} goes unreported: {}",
                thread_id,
                e,
                label,
                error
            );
            false
        }
    };
    if still_running {
        engine
            .event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                        error: error.to_string(),
                        spend_cap: None,
                        agent_exit: None,
                        provider_error: None,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                label,
            )
            .await;
    }
}

/// Re-exported so call sites that already import from `claude_code` don't have
/// to chase the enum into `types.rs`.
pub use crate::engine::types::StopReason;

pub use control::{RungPick, RungPickOutcome};

/// Returned by [`LucidosEngine::interrupt_agent`] when the session is already
/// idle/waiting — i.e. a Cancel (Stop) click that races a turn which already
/// finished. Callers (the `/claude-code/stop` handler) treat it as a no-op
/// (HTTP 200), not an error: there is nothing to interrupt. Lives here (not in
/// the private `control` submodule) so the API handler can reference it.
pub(crate) const SESSION_ALREADY_WAITING: &str = "Session is already waiting";

use crate::engine::session_claim_refusal::SESSION_STOPPING_MESSAGE;
pub(crate) use crate::engine::session_claim_refusal::{
    claim_refusal_message, claim_refusal_reason,
};
#[cfg(test)]
pub(crate) use crate::engine::session_claim_refusal::{
    APPLY_IN_PROGRESS_MESSAGE, DISCARD_IN_PROGRESS_MESSAGE,
};

/// Why Apply Now is refused while the thread is parked on a question. Applying
/// would send a hardening or merge prompt that overtakes the question card.
pub(crate) const QUESTION_OPEN_MESSAGE: &str =
    "This thread is waiting for your answer to its question. Answer it first, then apply";

/// Why Apply Now is refused when it could not check for a parked question. An
/// unanswered check is not a "no".
pub(crate) const QUESTION_UNKNOWN_MESSAGE: &str =
    "Could not check whether this thread is waiting for your answer. Try again";

/// What an in-session Discard found for its thread.
pub(crate) enum DiscardTarget {
    /// No session: the stale-session teardown handles the Discard.
    NoSession,
    /// The session, claimed for this Discard. Release it with `claimant`.
    Claimed {
        worktree: PathBuf,
        /// The branch the reset may rewind. The worktree is reset only while
        /// it is still checked out on it.
        branch: String,
        claimant: tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    },
}

pub(crate) fn claim_for_discard(
    sessions: &mut HashMap<Uuid, crate::engine::AgentSession>,
    thread_id: Uuid,
) -> Result<DiscardTarget, Box<dyn std::error::Error + Send + Sync>> {
    let Some(session) = sessions.get_mut(&thread_id) else {
        return Ok(DiscardTarget::NoSession);
    };
    if let Some(refusal) = stop_refusal(session) {
        return Err(refusal.into());
    }
    if session.pending_stop.is_some() {
        return Err(SESSION_STOPPING_MESSAGE.into());
    }
    let worktree = session
        .worktree_path
        .clone()
        .ok_or("No worktree for this session")?;
    let branch = session
        .branch_name
        .clone()
        .ok_or("No branch for this session")?;
    session.change_claim = Some(crate::engine::types::ChangeClaim::Discard);
    Ok(DiscardTarget::Claimed {
        worktree,
        branch,
        claimant: session.msg_tx.clone(),
    })
}

pub(crate) fn stop_refusal(session: &crate::engine::AgentSession) -> Option<&'static str> {
    if matches!(session.phase, crate::engine::SessionPhase::Applying) {
        return Some(THREAD_APPLYING_MESSAGE);
    }
    session.change_claim.map(claim_refusal_message)
}

/// A Stop, Discard or Archive while an apply resets the thread's worktree. No
/// loop listens to the entry, so a recorded stop would be lost.
pub(crate) const THREAD_APPLYING_MESSAGE: &str =
    "An apply is merging this thread's work. Try again when it finishes.";

pub use crate::engine::agent_host::SettleTerminal;

/// Emit a terminal event for a thread that still reads `running` after its
/// session or loop has gone away.
///
/// A Stop or Cancel button passes the clicking device as `actor`. Automatic
/// callers must pass `MessageOrigin::system()` to preserve sweep attribution.
/// The switch recovery gate requires both a device actor and `EngineShutdown`.
///
/// `StuckProjection` emits `ResponseAborted { StaleSettle }`.
/// `CanceledQuestion` and `UserStop` emit `ResponseCanceled { UserStop }`,
/// matching a live cancellation.
///
/// Returns `Ok(true)` after an emit and `Ok(false)` for a settled or missing
/// thread. Direct emission lets the HTTP caller observe errors.
pub(crate) async fn settle_stuck_running_thread(
    pool: &sqlx::PgPool,
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
    terminal: SettleTerminal,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if !thread_is_running(pool, thread_id).await? {
        return Ok(false);
    }

    let event = match terminal {
        SettleTerminal::StuckProjection => {
            crate::engine::thread_events::ThreadEvent::ResponseAborted {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause: crate::engine::thread_events::AbortCause::StaleSettle,
            }
        }
        SettleTerminal::CanceledQuestion | SettleTerminal::UserStop => {
            crate::engine::thread_events::ThreadEvent::ResponseCanceled {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause: crate::engine::thread_events::CancelCause::UserStop,
            }
        }
    };

    let mut meta = crate::engine::thread_events::EventMeta::with_actor(actor);
    if terminal != SettleTerminal::StuckProjection
        && thread_is_coding_agent(pool, thread_id).await?
    {
        meta.channel = Some(crate::engine::thread_events::EventChannel::ClaudeCode);
    }
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event,
        meta,
    })
    .await?;

    Ok(true)
}

async fn thread_is_coding_agent(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let is_coding_agent: Option<bool> =
        sqlx::query_scalar("SELECT is_coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    Ok(is_coding_agent == Some(true))
}

#[cfg(test)]
#[path = "../claude_code_tests/cache_status.rs"]
mod cache_status_tests;

#[cfg(test)]
#[path = "../claude_code_tests/settle.rs"]
mod settle_tests;

#[cfg(test)]
#[path = "../claude_code_tests/allowed_tools.rs"]
mod allowed_tools_tests;

#[cfg(test)]
#[path = "../claude_code_tests/session.rs"]
mod session_tests;
