//! Pure helpers and constructors used by the chat process flow. None touch
//! `LucidosEngine` — free functions and constants reusable across the chat
//! module.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

use crate::engine::thread_events::{
    EventChannel, EventMeta, MessageOrigin, ThreadEvent, TriggerInvocation,
};
use crate::engine::types::AgentSession;
use crate::engine::ThreadHandle;

/// Trigger context passed into the chat process flow. None for user-driven chat.
pub(super) struct TriggerContext {
    pub trigger_id: String,
    pub trigger_name: String,
    /// Stable kebab-case slug of the firing trigger, used to scope the
    /// per-trigger know-how listing in the system prompt to
    /// `data/triggers/{slug}/knowhow/`.
    pub slug: String,
    pub invocation: TriggerInvocation,
    pub go_to_review: bool,
    /// The trigger's declared **side-effect grant** (ADR 0002, Phase 5). The
    /// command guard consults it to decide whether an `IrreversibleDanger`
    /// command may run unattended; an ungranted side-effect fails the trigger.
    pub side_effect_grant: Vec<crate::engine::command_guard::SideEffectCategory>,
}

/// What restarts actually do to a chat thread, written as the chat-system
/// prompt block. The threading model: events are persisted in PostgreSQL, so
/// when the user returns to a thread its full history reloads on the next LLM
/// turn (see the `Load conversation history from DB` step in the same module).
/// What does NOT survive is anything in-flight — a streaming response, a
/// pending child callback, an autonomous loop. The earlier wording said the
/// thread itself was "wiped" and the LLM had "NO memory", which led the LLM
/// to instruct users to start a NEW thread after a restart instead of
/// returning to the existing one (observed in the
/// `Status of Authentication Migration` thread).
pub(super) const ENGINE_RESTART_RULE: &str = "ENGINE RESTARTS INTERRUPT IN-FLIGHT WORK, NOT THREAD MEMORY:\nThe thread itself survives engine restarts — every message, tool call, and response is persisted in the event store, and your next turn after a restart loads the full history. The user can return to THIS thread (don't tell them to start a new one) and re-prompt to wake you up; you will see what was discussed. What does NOT survive is in-flight work: a streaming response, a child thread you were waiting on a callback from, an autonomous loop, or a `sleep N minutes then check back` intent. The LLM (you) is not running between turns; once a restart cuts off a response, no continuation fires automatically. So: never promise to do something \"after the restart\", \"once it comes back up\", \"in a minute when it's live\", or to \"check back later\" — those promises require a process that no longer exists. If a restart is about to happen, tell the user to come back to this same thread and re-prompt.";

/// Stops the chat agent from bouncing yes/no "did you apply it? / did you
/// restart?" confirmations back at the user during the apply→restart loop.
/// The agent owns the grouped `changes` tool (action list/apply) and can probe served assets, so
/// it must inspect and verify state itself rather than asking. Motivated by a
/// font-fix session where, after applying a backend change, the agent kept
/// asking the user to confirm they'd applied/restarted and "does it match
/// now?" instead of checking. Pairs with `ENGINE_RESTART_RULE` (the agent
/// CANNOT restart the engine — only the user triggers the rebuild) and the
/// `ASK_USER_QUESTION_RULE` carve-out (ask only for genuine decisions). The
/// served-asset probe note records the gateway routing gotcha: bundled SDK /
/// static assets live under the workspace-prefixed route.
///
/// **Install-independent half.** Everything here holds wherever *changes*
/// exist — which includes an install with no Lucidos source checkout, since
/// app coding-agent threads still propose changes the user Applies. The
/// dev-only half (the "works on Lucidos constantly" framing and the
/// Rust-change rebuild/restart choreography) lives in
/// [`APPLY_VERIFY_DEV_ADDENDUM`] and is spliced only when a source checkout
/// exists — a packaged install never rebuilds the engine, and an app change
/// never restarts it, so promising that dance there is simply wrong.
pub(super) const APPLY_VERIFY_RULE: &str = "APPLYING & VERIFYING CHANGES — ACT AND VERIFY, DON'T ASK:\nYou have the `changes` tool (action 'list' / 'apply') — you can inspect and apply *changes* (coding-agent-proposed branches awaiting Apply) yourself, so NEVER bounce a yes/no question back to the user about state you can check.\n- NEVER ask \"have you applied it?\" / \"did you apply the change?\" — call `changes` with action 'list' and read whether it is `pending` or `applied`. If the user asked for it to be applied, call `changes` with action 'apply' directly; don't ask permission for the thing they just told you to do.\n- PROBE SERVED ASSETS UNDER THE WORKSPACE-PREFIXED ROUTE: bundled SDK / static assets are served under the workspace route, e.g. `https://<host>/<workspace>/api/v1/sdk-iframe.css` — NOT a bare `/api/v1/...` path. A bare path makes the gateway resolve the first segment (`api`) as a workspace name and 404 with \"unknown workspace 'api'\".\n- NEVER end an apply/restart loop with a confirmation question like \"does it look right now?\" or \"does it match?\". State what you verified, objectively. Reserve `ask_user_question` for a genuine decision the user has to make — not to confirm a step you could check yourself.";

/// The half of the apply/verify rule that is only true on an install launched
/// from a Lucidos source checkout: the engine rebuild+restart choreography for
/// a Rust change, and the "this user does this daily" framing.
///
/// Spliced by `build_chat_system_prompt` only when
/// [`crate::paths::has_lucidos_source`] is true. On a packaged install there is
/// no engine rebuild at all (updates ship through the app updater) and app
/// changes never restart the engine, so an agent reciting this would be
/// instructing the user through a flow that does not exist — which is precisely
/// what happened in the reported failure.
pub(super) const APPLY_VERIFY_DEV_ADDENDUM: &str = "\n- The user works on Lucidos constantly and knows the apply/restart/reload dance cold — do not re-explain it, and do not ask them to confirm they did it.\n- NEVER ask \"did you restart?\" / \"is the engine back up yet?\". You CANNOT restart the engine yourself — only the user can trigger the rebuild/restart, and Lucidos shows them the restart toast. So for a change that touches Rust/backend files (`requires_restart: true`): (a) apply it, (b) state plainly that a restart is required and that the USER must trigger it (don't promise the change is live immediately), then (c) VERIFY whether the new build is live by probing the served asset or behavior YOURSELF — e.g. `curl` the served file — rather than asking. If the probe still shows the old output, say so factually (\"the engine is still serving the pre-restart build\") instead of asking \"did you restart?\".";

/// Build the TriggerStarted thread-event + meta for a scheduler-fired trigger
/// run. Extracted as a pure function so the wiring rule "the `config.id`
/// passed in is what gets stamped into both `TriggerStarted.trigger_id` and
/// `EngineReason::Scheduler.trigger_id`" stays unit-testable without standing
/// up the LLM that `process_trigger` runs through.
pub(super) fn build_trigger_started_event(
    trigger_id: &str,
    trigger_name: &str,
    invocation: &TriggerInvocation,
    user_message: &str,
    go_to_review: bool,
) -> (ThreadEvent, EventMeta) {
    use crate::engine::thread_events::EngineReason;
    (
        ThreadEvent::TriggerStarted {
            trigger_id: trigger_id.to_string(),
            trigger_name: Some(trigger_name.to_string()),
            prompt: Some(user_message.to_string()),
            invocation: Some(invocation.clone()),
            // Scheduler-fired triggers always carry Engine origin so the
            // route popover can render "Engine · Scheduled · <name>".
            origin: Some(MessageOrigin::engine(EngineReason::Scheduler {
                trigger_id: trigger_id.to_string(),
                trigger_name: Some(trigger_name.to_string()),
            })),
            go_to_review,
        },
        EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    )
}

/// Wall-clock cap on auxiliary Flash calls in the chat slow path
/// (history summarization, query classification). The Vertex non-streaming
/// client itself allows 900s, so without this an occasional Flash hang would
/// silently stall the chat between MessageReceived and the first agentic step
/// — exactly the "stuck thread" symptom users observe. Defaults are safe: on
/// timeout we fall back to truncation / "needs everything" classification.
const AUX_LLM_TIMEOUT: Duration = Duration::from_secs(30);

/// Run the Flash summarization future under `AUX_LLM_TIMEOUT`. On error or
/// timeout, fall back to the truncation placeholder used elsewhere in the
/// chat path.
pub(super) async fn summarize_or_fallback<F>(fut: F, older_len: usize) -> String
where
    F: Future<Output = Result<String, Box<dyn std::error::Error + Send + Sync>>>,
{
    let truncation_fallback = || format!("({} earlier messages not shown)", older_len);
    match tokio::time::timeout(AUX_LLM_TIMEOUT, fut).await {
        Ok(Ok(s)) => {
            log!(
                "[Chat] Compressed {} older messages into {} char summary",
                older_len,
                s.len()
            );
            s
        }
        Ok(Err(e)) => {
            log!(
                "[Chat] History summarization failed, falling back to truncation: {}",
                e
            );
            truncation_fallback()
        }
        Err(_) => {
            log!(
                "[Chat] History summarization timed out ({}s), falling back to truncation",
                AUX_LLM_TIMEOUT.as_secs()
            );
            truncation_fallback()
        }
    }
}

/// Run the Flash classification future under `AUX_LLM_TIMEOUT`. On error or
/// timeout, fall back to `QueryClassification::default()` ("needs everything").
pub(super) async fn classify_or_fallback<F>(fut: F) -> crate::memory::QueryClassification
where
    F: Future<
        Output = Result<
            crate::memory::QueryClassification,
            Box<dyn std::error::Error + Send + Sync>,
        >,
    >,
{
    match tokio::time::timeout(AUX_LLM_TIMEOUT, fut).await {
        Ok(Ok(c)) => {
            log!("[Chat] Query classification: needs_memory={}, needs_file_list={}, needs_credentials={}, sub_queries={:?}",
                c.needs_memory, c.needs_file_list, c.needs_credentials, c.sub_queries);
            c
        }
        Ok(Err(e)) => {
            log!(
                "[Chat] Query classification failed (defaulting to all): {}",
                e
            );
            crate::memory::QueryClassification::default()
        }
        Err(_) => {
            log!(
                "[Chat] Query classification timed out ({}s), defaulting to all",
                AUX_LLM_TIMEOUT.as_secs()
            );
            crate::memory::QueryClassification::default()
        }
    }
}

/// Build the per-trigger know-how section for the system prompt of trigger
/// threads. Lists files under `<workspace>/data/triggers/<slug>/knowhow/`,
/// scoped so threads of OTHER triggers never see this trigger's items.
/// Returns empty when the trigger has no knowhow files (caller appends nothing).
///
/// IDs use the global `triggers/<slug>/<file>` namespace so the listed
/// `load_knowhow` calls resolve via the same fallback resolution
/// (`load_with_fallback`) regular ids do.
pub(super) fn build_trigger_knowhow_section(triggers_dir: &std::path::Path, slug: &str) -> String {
    let summaries = crate::core::KnowhowStore::load_trigger_summaries(triggers_dir, slug);
    if summaries.is_empty() {
        return String::new();
    }
    let mut section = String::from(
        "\n\n## Trigger Know-how (this trigger only)\n\n\
        Files below belong to THIS trigger and are likely relevant. \
        Use `load_knowhow` with the id shown.\n\n",
    );
    for kh in &summaries {
        section.push_str(&format!(
            "- **{}** (id: `triggers/{}/{}`): {}\n",
            kh.name, slug, kh.id, kh.description
        ));
    }
    section
}

/// Wording must explicitly state no restart is needed; without that the LLM
/// reads "shipped with the engine" as "baked into the binary" and tells
/// users to restart. See the regression test for the full backstory.
pub(super) fn build_system_knowhow_section(
    summaries: &[crate::core::knowhow::KnowhowSummary],
) -> String {
    if summaries.is_empty() {
        return String::new();
    }
    let mut section = String::from(
        "\n\n## System Knowhow\n\n\
        Authoritative reference shipped with the Lucidos engine, read live from disk on \
        every chat turn — edits land immediately after the user clicks Apply, no engine \
        restart required. Use `load_knowhow` with the prefixed id (e.g. \
        `system-knowhow/<id>`) to load full content.\n\n",
    );
    for sd in summaries {
        section.push_str(&format!(
            "- **{}** (id: `system-knowhow/{}`): {}\n",
            sd.name, sd.id, sd.description
        ));
    }
    section
}

/// Bridge the race between a chat handler that has already taken
/// `active_threads[thread_id]` (and is mid-spawn of a Claude Code subprocess) and the
/// fast-path on a follow-up POST that needs `agent_sessions[thread_id]` to
/// exist before it can route via `msg_tx`.
///
/// Without this bridge, a follow-up POST that arrives in the 1-10s window
/// between `register_thread` and `agent_sessions.insert()` falls to the slow
/// path, blocks 60s in `register_thread_queued`, then force-evicts the
/// still-spawning CC. Symptoms: `ResponseAborted (cause=safety_net)` on a
/// fresh CC turn, plus a brand-new replacement spawn that often dies
/// immediately because its resume points at the just-killed conversation.
///
/// Returns `true` when `agent_sessions[thread_id]` is populated with a live
/// session (see `AgentSession::is_live` — a phantom left by a dropped run
/// future does not count) before the deadline. Returns `false` when:
///   - `active_threads[thread_id]` clears before population (chat handler
///     bailed without registering a session — fall through to a fresh spawn);
///   - the deadline elapses (caller falls through to slow path).
///
/// `poll_interval` keeps the busy-loop bounded — at the default 100ms a
/// 30-second deadline is 300 mutex acquisitions, dwarfed by the cost of CC
/// startup itself.
pub(super) async fn wait_for_cc_session_alive(
    agent_sessions: &TokioMutex<HashMap<Uuid, AgentSession>>,
    active_threads: &StdMutex<HashMap<Uuid, ThreadHandle>>,
    thread_id: Uuid,
    deadline: Duration,
    poll_interval: Duration,
) -> bool {
    let start = std::time::Instant::now();
    let mut logged_wait = false;
    loop {
        {
            let guard = agent_sessions.lock().await;
            if let Some(s) = guard.get(&thread_id) {
                if s.is_live() {
                    return true;
                }
            }
        }
        // No chat handler is mid-spawn anymore — no point in waiting.
        let chat_active = active_threads.lock().unwrap().contains_key(&thread_id);
        if !chat_active {
            return false;
        }
        if !logged_wait {
            log!(
                "[Chat] Thread {} active in chat but no agent session yet — \
                 bridging spawn race (up to {}s)",
                thread_id,
                deadline.as_secs()
            );
            logged_wait = true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// Backstop on how long the follow-up fast-path blocks waiting for an
/// interrupted turn to reach a boundary before routing the follow-up anyway.
/// The graceful interrupt (Codex `turn/interrupt`, Claude Code
/// `control_request:interrupt`) normally idles the turn in ~1-2s; the engine's
/// 8s interrupt-escalation hard-kill guarantees `process_exited` well under
/// this cap, so this only fires in a pathological non-idling case.
pub(super) const REDIRECT_INTERRUPT_MAX_WAIT: Duration = Duration::from_secs(30);

/// Whether a follow-up routed to a live coding-agent session should first
/// interrupt the in-flight turn ("interrupt-and-redirect") instead of being
/// forwarded as-is.
///
/// The two backends reach the same rule from opposite directions, so the
/// asymmetry in `urgent` is structural rather than a policy choice:
///
/// - **Codex redirects whether or not the caller said urgent.** Its app-server
///   / exec protocols accept input only at a turn boundary (`turn/start` needs
///   no turn in flight), so a mid-turn follow-up would otherwise sit invisibly
///   in the driver's queue behind a long turn. Queueing is not a gentler
///   option there, it is an invisible one, so `urgent` is a no-op for Codex and
///   the LLM tool description says so.
/// - **Claude Code redirects only when urgent.** CC accepts a follow-up
///   mid-turn (the driver writes it to stdin at once), so a plain follow-up is
///   forwarded as-is and steers the child at its next turn boundary, which is
///   what keeps a benign steer from throwing away an in-flight build. But CC
///   surfaces a queued stdin message ONLY at that boundary, and a blocking tool
///   call pushes the boundary out by up to its own timeout: on 2026-08-06 a
///   child parked in `TaskOutput(block: true, timeout: 600000)` did not read a
///   STOP for nine and a half minutes. `urgent` buys the interrupt for the
///   messages that cannot wait for it.
///
/// Pure so the routing rule is unit-tested without standing up a session.
///
/// - `is_in_flight`: the session is alive and NOT at a turn boundary
///   (`AgentSession::is_in_flight`). A follow-up to an idle-but-alive session
///   is picked up immediately on either backend, so there is nothing to
///   interrupt and urgency does not manufacture a turn to stop.
/// - `is_user_message`: the input is a genuine user follow-up, not an
///   engine-internal child-wake (`AgentInputKind::WakeFromChild`). Child-wakes
///   never redirect, urgent or not: they resume a thread that is waiting on a
///   child.
/// - `urgent`: the caller marked the follow-up as one the child must act on
///   now, accepting that its current turn ends. See `FollowUpDelivery`.
pub(super) fn should_redirect_followup(
    coding_agent: crate::runtime::CodingAgent,
    is_in_flight: bool,
    is_user_message: bool,
    urgent: bool,
) -> bool {
    if !is_in_flight || !is_user_message {
        return false;
    }
    match coding_agent {
        crate::runtime::CodingAgent::Codex => true,
        crate::runtime::CodingAgent::ClaudeCode => urgent,
    }
}

/// Arm the interrupt-and-redirect for a follow-up, operating on the
/// already-locked `agent_sessions` map. When `thread_id`'s session is a turn in
/// flight that should be redirected (per `should_redirect_followup`), this:
///   1. **Pre-counts the follow-up** (`pending_followups += 1`) so the
///      interrupted turn's idle reads `> 1` and `terminate_decision` keeps the
///      subprocess alive (`pending_followups` starts at 1 for the initial
///      turn). The caller's normal `msg_tx` send increments again for the
///      follow-up's own expected `Result`.
///   2. **Stamps the redirecting device** onto `cancel_actor` — the same slot
///      `interrupt_agent` uses for the Stop button; the run_session interrupt
///      arm drains it onto `ResponseCanceled.actor`.
///   3. **Fires the interrupt** so the live turn ends as a resumable `Canceled`
///      boundary.
///
/// Returns the session's `idle_notify` so the caller can wait for the turn to
/// wind down before emitting the follow-up's `MessageReceived` (correct event
/// ordering). Returns `None` when no redirect applies (a plain Claude Code
/// follow-up, an idle session, a child-wake, or no live session) and the caller
/// routes the follow-up normally.
///
/// Pure over the map (no engine, no async) so the arming behavior is unit-tested
/// with a fake session, like `wait_for_cc_session_alive`.
pub(super) fn arm_followup_redirect(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    is_user_message: bool,
    urgent: bool,
    origin: &Option<MessageOrigin>,
) -> Option<std::sync::Arc<tokio::sync::Notify>> {
    let s = sessions.get_mut(&thread_id)?;
    if !should_redirect_followup(s.coding_agent, s.is_in_flight(), is_user_message, urgent) {
        return None;
    }
    // Keep the interrupted turn's idle from terminating the subprocess:
    // `terminate_decision` keeps it alive only when the pre-swap count is `> 1`.
    // A normal turn pre-counts its own Result as 1 at session creation, so a
    // single `+1` reaches 2; a silent-resume / warm-up turn starts at 0, so a
    // lone `+1` would land at 1 → `Terminate`, killing the queued follow-up.
    // Bump to at least 2 in that case. The follow-up's own expected Result is
    // counted separately by the caller's `msg_tx` send. Race-free w.r.t. the
    // idle handler's lock-free `swap`: the run loop flips `is_waiting` under the
    // same `agent_sessions` lock we hold here, and `is_in_flight()` was just
    // observed true, so its `swap` happens strictly after we release.
    if s.pending_followups
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
        == 0
    {
        s.pending_followups
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    s.cancel_actor = origin.clone();
    // Mark this interrupt as a redirect (not a Stop click) so the run_session
    // interrupt arm classifies the interrupted turn as
    // `CancelCause::SupersededByFollowup` — rendered neutrally — instead of
    // `UserStop`. Drained by `take_session_redirect_followup`.
    s.redirect_followup = true;
    s.interrupt.notify_one();
    Some(s.idle_notify.clone())
}

#[cfg(test)]
#[path = "process_helpers_tests.rs"]
mod process_helpers_tests;
