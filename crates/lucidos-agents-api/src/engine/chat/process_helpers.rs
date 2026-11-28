use std::collections::HashMap;
use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

use crate::engine::thread_events::{
    EventChannel, EventMeta, MessageOrigin, RetryOf, ThreadEvent, TriggerInvocation,
};
use crate::engine::types::AgentSession;

pub(super) struct TriggerContext {
    pub trigger_id: String,
    pub trigger_name: String,
    pub slug: String,
    pub go_to_review: bool,
    pub mcp_servers: Vec<String>,
    pub turn: TriggerTurn,
}

pub(super) enum TriggerTurn {
    Fire(TriggerFire),
    Resume,
}

pub(super) struct TriggerFire {
    pub retry_of: Option<RetryOf>,
    pub invocation: TriggerInvocation,
    pub queue_entry_id: Uuid,
    pub continue_key: Option<crate::triggers::continue_key::ContinueKeyClaim>,
    pub actor_email: Option<String>,
}

pub(super) const ENGINE_RESTART_RULE: &str = "ENGINE RESTARTS INTERRUPT IN-FLIGHT WORK, NOT THREAD MEMORY:\nThe thread survives a restart: every message, tool call and response is persisted, and your next turn loads the full history. Send the user back to THIS thread to re-prompt, never to a fresh one. In-flight work does NOT survive: a streaming response, an autonomous loop, a `sleep N minutes then check back` intent. You do not run between turns, and nothing resumes on its own once a restart cuts a response off, so never promise anything \"after the restart\", \"once it comes back up\", or \"check back later\".";

pub(super) const APPLY_VERIFY_RULE: &str = "APPLYING & VERIFYING CHANGES, ACT AND VERIFY RATHER THAN ASK:\nYou have the `changes` tool (action 'list' / 'apply'), so never bounce a yes/no question back at the user about state you can check yourself.\n- NEVER ask \"have you applied it?\". Call action 'list' and read whether it is `pending` or `applied`. If they asked for it to be applied, call action 'apply' rather than asking permission for what they just told you to do.\n- PROBE SERVED ASSETS UNDER THE WORKSPACE-PREFIXED ROUTE, e.g. `https://<host>/<workspace>/api/v1/sdk-iframe.css`, never a bare `/api/v1/...`: a bare path makes the gateway read the first segment (`api`) as a workspace name and 404 with \"unknown workspace 'api'\".\n- State what you verified, objectively, and never close an apply loop with \"does it look right now?\". Reserve `ask_user_question` for a genuine decision, not to confirm a step you could check.";

pub(super) const LOOK_BEFORE_ASSESSING_RULE: &str = "ESTABLISH THE STATE BEFORE YOU FORM AN ASSESSMENT:\nAsked to judge, compare or advise on something of theirs, find out how it is ACTUALLY going before deciding what you think. __MEMORY_LOOKUP__ An ORDER, not a review at the end: once you have a view you will not go looking.\n- NEVER assert something has not happened, does not exist, or was never discussed, unless you looked. Absence from your context is not absence from their history.\n- For answering, not browsing: stop once you have what you need.\nIn EVERY reply, not only when asked to judge: say what another thread, change or phase is doing NOW (waiting, running, blocked, done) only from a read in THIS turn: the [CHILD THREADS] block, `events`, `threads` or `changes`. Otherwise say it in the past tense with its time (\"At 13:36 it reported waiting\"). Your earlier messages and its earlier reports are history. Read what a thread is doing from liveness.state. status: idle means no turn is open. A waiting thread wakes itself on the waits liveness names.";

pub(super) const MEMORY_LOOKUP_CLASSIC: &str = "Injected memories come first; when they are thin, the `memory` and `threads` tools both have a 'search'.";

pub(super) const MEMORY_LOOKUP_TREE: &str = "Your memory views come first; when they are thin, the `recall` and `threads` tools both have a 'search'.";

/// INVARIANT: The half of the apply/verify rule that is only true on an install launched
/// from a Lucidos source checkout: the engine rebuild+restart choreography for
/// a Rust change, and the "this user does this daily" framing.
///
/// Spliced by `build_chat_system_prompt` only when
/// [`crate::paths::has_lucidos_source`] is true. On a packaged install there is
/// no engine rebuild at all (updates ship through the app updater) and app
/// changes never restart the engine, so an agent reciting this would be
/// instructing the user through a flow that does not exist — which is precisely
/// what happened in the reported failure.
///
/// **It supplies no sentence for an unverified outcome, deliberately.** It used
/// to close with a parenthetical example of what to say when the probe still
/// showed the old build, and on 2026-08-09 a thread wrote that sentence
/// verbatim about an engine the user HAD restarted, without running any probe.
/// A pre-written conclusion is the cheapest thing in the prompt to reach for,
/// so the rule now names where the answer actually is (the per-turn ENGINE
/// BUILD section, which states it as a fact) and leaves the wording to the
/// agent. The served-asset probe stays, scoped to what it can really answer: a
/// change that altered a served asset. It says nothing about a backend-only
/// change, which is exactly the kind the failure involved.
pub(super) const APPLY_VERIFY_DEV_ADDENDUM: &str = "\n- The user works on Lucidos constantly and knows the apply/restart/reload dance cold, so do not re-explain it or ask them to confirm they did it.\n- NEVER ask \"did you restart?\", and never infer it from what you applied earlier: the ENGINE BUILD section states it, fresh every turn. They restart whenever they like, including mid-turn.\n- You cannot restart the engine yourself. So after applying a `requires_restart: true` change, say what that section says, live or waiting on their switch, and never assert either without reading it.\n- Probing a served asset verifies a change that ALTERED a served asset, nothing else. A backend-only change alters none, so the section is the whole answer.";

pub(super) fn build_trigger_started_event(
    tc: &TriggerContext,
    user_message: &str,
    selection: &crate::core::ResolvedModelSelection,
) -> Option<(ThreadEvent, EventMeta)> {
    use crate::engine::thread_events::EngineReason;
    let TriggerTurn::Fire(fire) = &tc.turn else {
        return None;
    };
    Some((
        ThreadEvent::TriggerStarted {
            retry_of: fire.retry_of.clone(),
            trigger_id: tc.trigger_id.clone(),
            trigger_name: Some(tc.trigger_name.clone()),
            prompt: Some(user_message.to_string()),
            invocation: Some(fire.invocation.clone()),
            origin: Some(MessageOrigin::engine(EngineReason::Scheduler {
                trigger_id: tc.trigger_id.clone(),
                trigger_name: Some(tc.trigger_name.clone()),
                actor_email: fire.actor_email.clone(),
            })),
            go_to_review: tc.go_to_review,
            model: selection.model.clone(),
            reasoning_effort: selection.reasoning_effort.clone(),
            provider: selection.provider.clone(),
            continue_key: fire.continue_key.as_ref().map(|claim| claim.key.clone()),
        },
        EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    ))
}

const SUMMARY_MIN_CHARS_PER_TURN: usize = 60;

const SUMMARY_MIN_CHARS_CAP: usize = 600;

pub(super) fn summary_is_too_thin(chars: usize, covered_turns: usize) -> bool {
    let required = (covered_turns * SUMMARY_MIN_CHARS_PER_TURN).min(SUMMARY_MIN_CHARS_CAP);
    chars < required
}

pub(super) async fn summarize_or_none<F>(
    fut: F,
    older_len: usize,
    deadline: Duration,
) -> Option<String>
where
    F: Future<Output = Result<String, Box<dyn std::error::Error + Send + Sync>>>,
{
    let started = std::time::Instant::now();
    match tokio::time::timeout(deadline, fut).await {
        Ok(Ok(s))
            if !s.trim().is_empty()
                && !summary_is_too_thin(s.trim().chars().count(), older_len) =>
        {
            log_legacy!(
                "[Chat] Compressed {} older assistant turns into {} char summary in {:?}",
                older_len,
                s.trim().chars().count(),
                started.elapsed()
            );
            Some(s)
        }
        Ok(Ok(s)) => {
            log_legacy!(
                "[Chat] History summarization returned {} chars for {} turns, too thin to cache; keeping any cached summary",
                s.trim().chars().count(),
                older_len
            );
            None
        }
        Ok(Err(e)) => {
            log_legacy!(
                "[Chat] History summarization failed after {:?}, keeping any cached summary: {}",
                started.elapsed(),
                e
            );
            None
        }
        Err(_) => {
            log_legacy!(
                "[Chat] History summarization timed out ({}s), keeping any cached summary",
                deadline.as_secs()
            );
            None
        }
    }
}

pub(super) const FORCE_QUERY_CLASSIFICATION_ENV: &str = "LUCIDOS_FORCE_QUERY_CLASSIFICATION";

pub(super) fn forced_classification(
    raw: Option<&str>,
) -> Option<crate::memory::QueryClassification> {
    let raw = raw?.trim();
    if raw.eq_ignore_ascii_case("all") {
        return Some(crate::memory::QueryClassification::default());
    }
    if raw.eq_ignore_ascii_case("none") {
        return Some(crate::memory::QueryClassification {
            needs_memory: false,
            needs_file_list: false,
            needs_credentials: false,
            sub_queries: vec![],
        });
    }
    None
}

pub(super) fn pinned_classification() -> Option<crate::memory::QueryClassification> {
    static PIN: OnceLock<Option<crate::memory::QueryClassification>> = OnceLock::new();
    let pinned = PIN
        .get_or_init(|| {
            let raw = std::env::var(FORCE_QUERY_CLASSIFICATION_ENV).ok();
            let parsed = forced_classification(raw.as_deref());
            if parsed.is_none() {
                if let Some(value) = raw.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                    log_legacy!(
                        "[Chat] {} is set to {:?}, which is neither `all` nor `none`. Ignoring \
                         it and classifying normally.",
                        FORCE_QUERY_CLASSIFICATION_ENV,
                        value
                    );
                }
            }
            parsed
        })
        .clone()?;
    log_classification(&pinned, true);
    Some(pinned)
}

/// INVARIANT: The one line a turn writes about its classification.
///
/// Shared so a pinned turn logs at the same place an unpinned one does, and
/// says which it was. A pinned run that logged nothing would read as a run
/// where classification never happened.
fn log_classification(c: &crate::memory::QueryClassification, pinned: bool) {
    let pin = if pinned {
        format!(" (pinned by {FORCE_QUERY_CLASSIFICATION_ENV})")
    } else {
        String::new()
    };
    log_legacy!("[Chat] Query classification{}: needs_memory={}, needs_file_list={}, needs_credentials={}, sub_queries={:?}",
        pin, c.needs_memory, c.needs_file_list, c.needs_credentials, c.sub_queries);
}

/// INVARIANT: Run the Flash classification future under `deadline`. On error or timeout,
/// fall back to `QueryClassification::default()` ("needs everything").
///
/// `deadline` comes from `engine::aux_purpose::budget_for`. Classification runs
/// on every turn with the user waiting behind it, so its budget is the short
/// one, not the summariser's.
pub(super) async fn classify_or_fallback<F>(
    fut: F,
    deadline: Duration,
) -> crate::memory::QueryClassification
where
    F: Future<
        Output = Result<
            crate::memory::QueryClassification,
            Box<dyn std::error::Error + Send + Sync>,
        >,
    >,
{
    let started = std::time::Instant::now();
    match tokio::time::timeout(deadline, fut).await {
        Ok(Ok(c)) => {
            log_classification(&c, false);
            log_legacy!("[Chat] Query classification took {:?}", started.elapsed());
            c
        }
        Ok(Err(e)) => {
            log_legacy!(
                "[Chat] Query classification failed after {:?} (defaulting to all): {}",
                started.elapsed(),
                e
            );
            crate::memory::QueryClassification::default()
        }
        Err(_) => {
            log_legacy!(
                "[Chat] Query classification timed out ({}s), defaulting to all",
                deadline.as_secs()
            );
            crate::memory::QueryClassification::default()
        }
    }
}

pub(super) fn build_trigger_knowhow_section(triggers_dir: &std::path::Path, slug: &str) -> String {
    let summaries = crate::core::KnowhowStore::load_trigger_summaries(triggers_dir, slug);
    if summaries.is_empty() {
        return String::new();
    }
    let mut section = String::from(
        "[TRIGGER KNOW-HOW]\n\
        Files below belong to THIS trigger and are likely relevant. \
        Use `load_knowhow` with the id shown.\n\n",
    );
    for kh in &summaries {
        section.push_str(&format!(
            "- **{}** (id: `triggers/{}/{}`): {}\n",
            kh.name,
            slug,
            kh.id,
            super::process::workspace_prompt_footprint::routing_description(&kh.description)
        ));
    }
    section.push_str("[END TRIGGER KNOW-HOW]");
    section
}

pub(super) fn build_removed_knowhow_section(
    removed: &[crate::triggers::affected::RemovedKnowhow],
) -> String {
    if removed.is_empty() {
        return String::new();
    }
    let mut section =
        String::from("[REMOVED KNOW-HOW]\nA plugin removed know-how this trigger relied on:\n");
    for r in removed {
        section.push_str(&format!("- `{}`, removed by {}\n", r.path, r.removed_by));
    }
    section.push_str(
        "Do not invent a replacement recipe silently. Look for current know-how with \
         `load_knowhow`. Say in your result that this trigger's know-how was removed, \
         and suggest the user edit the trigger.\n[END REMOVED KNOW-HOW]",
    );
    section
}

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

pub(super) const REDIRECT_INTERRUPT_MAX_WAIT: Duration = Duration::from_secs(30);

pub(super) fn should_redirect_followup(
    coding_agent: crate::runtime::CodingAgent,
    is_in_flight: bool,
    is_user_message: bool,
    urgent: bool,
    can_steer: bool,
) -> bool {
    if !is_in_flight || !is_user_message {
        return false;
    }
    match coding_agent {
        crate::runtime::CodingAgent::Codex => urgent || !can_steer,
        crate::runtime::CodingAgent::ClaudeCode => urgent,
    }
}

pub(super) fn arm_followup_redirect(
    sessions: &mut HashMap<Uuid, AgentSession>,
    thread_id: Uuid,
    is_user_message: bool,
    urgent: bool,
    origin: &Option<MessageOrigin>,
) -> Option<std::sync::Arc<tokio::sync::Notify>> {
    let s = sessions.get_mut(&thread_id)?;
    if !s.is_running() || s.in_ceiling_reentry {
        return None;
    }
    if !should_redirect_followup(
        s.coding_agent,
        s.is_in_flight(),
        is_user_message,
        urgent,
        s.can_steer,
    ) {
        return None;
    }
    // INVARIANT: Keep the interrupted turn's idle from terminating the subprocess. A flag
    // rather than a count, because the thing being expressed is a promise ("a
    // message is coming"), not a quantity, and it has to hold across a window in
    // which the message provably is not in `msg_rx` yet.
    // The run loop flips `is_waiting` under the lock we hold here. We just saw the
    // turn in flight, so the idle decision runs strictly after we release. The
    // turn may still end on its own first, with its `Result` already in. Then
    // `mark_turn_boundary` retires the flag, the actor and the permit below, and
    // leaves only this promise.
    s.redirect_followup_pending = true;
    s.cancel_actor = origin.clone();
    s.redirect_cause = Some(crate::engine::thread_events::CancelCause::SupersededByFollowup);
    s.interrupt.notify_one();
    Some(s.idle_notify.clone())
}

pub(super) async fn follow_up_keeps_open_question(
    pool: &sqlx::PgPool,
    agent_sessions: &TokioMutex<HashMap<Uuid, AgentSession>>,
    thread_id: Uuid,
    pre_emitted_origin: Option<super::PreEmittedOrigin>,
) -> bool {
    reentry_reaches_live_session(agent_sessions, thread_id, pre_emitted_origin).await
        && crate::engine::agent_question::lookup_active_question_tool_use_id(pool, thread_id)
            .await
            .is_some()
}

pub(super) async fn reentry_reaches_live_session(
    agent_sessions: &TokioMutex<HashMap<Uuid, AgentSession>>,
    thread_id: Uuid,
    pre_emitted_origin: Option<super::PreEmittedOrigin>,
) -> bool {
    if !pre_emitted_origin.is_some_and(|o| o.is_engine_reentry()) {
        return false;
    }
    agent_sessions
        .lock()
        .await
        .get(&thread_id)
        .is_some_and(|s| s.is_live())
}

pub(super) fn chat_follow_up_supersedes_cards(
    mode: crate::engine::thread_events::ActorMode,
    has_live_turn: bool,
) -> bool {
    mode == crate::engine::thread_events::ActorMode::Human || !has_live_turn
}

#[cfg(test)]
#[path = "process_helpers_tests.rs"]
mod process_helpers_tests;
