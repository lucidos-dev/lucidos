//! Pure helpers and constructors used by the chat process flow. None touch
//! `LucidosEngine` — free functions and constants reusable across the chat
//! module.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent, TriggerInvocation};
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
            log!("[Chat] Query classification failed (defaulting to all): {}", e);
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
pub(super) fn build_trigger_knowhow_section(
    triggers_dir: &std::path::Path,
    slug: &str,
) -> String {
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
/// (non-`process_exited`) session before the deadline. Returns `false` when:
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
                if !s.process_exited {
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

#[cfg(test)]
#[path = "process_helpers_tests.rs"]
mod process_helpers_tests;
