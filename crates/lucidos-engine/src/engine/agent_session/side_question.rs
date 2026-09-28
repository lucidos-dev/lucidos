//! Side questions (`/btw`) in Claude Code threads: a quick question answered
//! from the session's context, beside any running turn, with no tools.
//!
//! Each ask is recorded as side-question thread events, so its card survives a
//! reload and shows on every device. No agent ever reads them: every generic
//! event reader excludes them (ADR 0320).

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::types::AgentSession;
use crate::engine::LucidosEngine;
use crate::runtime::{CodingAgent, SideQuestionRequest};

/// Why a side question got no answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SideQuestionFailure {
    /// The thread cannot take side questions. The text tells the user why.
    Refused(&'static str),
    /// A side question with this id was already asked. The first ask stands.
    AlreadyAsked,
    /// Nothing asked with this id on the thread, so there is nothing to dismiss.
    NotAsked,
    /// Claude Code could not answer. The text says what went wrong.
    Failed(String),
}

/// What startup recovery records for an ask a restart left unanswered.
pub(crate) const INTERRUPTED_BY_RESTART: &str = "Interrupted by a restart. Ask again.";

pub(crate) const EMPTY_QUESTION: &str = "Type a question after /btw.";
pub(crate) const NOT_CODING_AGENT_THREAD: &str = "Side questions work only in Claude Code threads.";
pub(crate) const CODEX_UNSUPPORTED: &str =
    "Side questions are not available in Codex threads. Send it as a normal message instead.";
const NO_SESSION_YET: &str =
    "This thread has no Claude Code session yet. Wait for its first reply, then ask again.";

/// Refusal for a `/btw` message on the normal chat route of a coding-agent
/// thread. Sent there, it would become a real turn in the main session.
pub(crate) const SIDE_QUESTION_ON_CHAT_ROUTE: &str =
    "A /btw side question is never sent to the main session. Ask it through POST /api/v1/coding-agents/side-question.";

/// Serializes the duplicate-id check with the ask's record. Held only for
/// those two quick writes, never across the answer.
static ASK_ADMISSION: Mutex<()> = Mutex::const_new(());

/// Whether `message` is a side question: `/btw` as its first word.
pub(crate) fn is_side_question(message: &str) -> bool {
    message
        .trim_start()
        .strip_prefix("/btw")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// The refusal for a thread, from its `thread_summaries` row
/// `(source, coding_agent)`, or `None` when it can take a side question.
fn refusal_for(row: Option<(String, Option<String>)>) -> Option<&'static str> {
    match row {
        Some((source, coding_agent)) if source == "claude_code" => {
            match CodingAgent::parse(coding_agent.as_deref().unwrap_or_default()) {
                CodingAgent::ClaudeCode => None,
                CodingAgent::Codex => Some(CODEX_UNSUPPORTED),
            }
        }
        _ => Some(NOT_CODING_AGENT_THREAD),
    }
}

async fn check_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<(), SideQuestionFailure> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT source, coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| SideQuestionFailure::Failed(format!("Could not read the thread: {e}")))?;
    match refusal_for(row) {
        Some(refusal) => Err(SideQuestionFailure::Refused(refusal)),
        None => Ok(()),
    }
}

/// Ask the thread's live Claude Code process. `Ok(None)` means there is none,
/// or it ended before it answered, so the caller asks a cold process instead.
async fn ask_live(
    sessions: &Mutex<HashMap<Uuid, AgentSession>>,
    thread_id: Uuid,
    question: &str,
    deadline: tokio::time::Instant,
) -> Result<Option<String>, SideQuestionFailure> {
    let sender = sessions
        .lock()
        .await
        .get(&thread_id)
        .filter(|session| !session.process_exited)
        .and_then(|session| session.side_question_tx.clone());
    let Some(sender) = sender else {
        return Ok(None);
    };
    let (reply, answer) = tokio::sync::oneshot::channel();
    let request = SideQuestionRequest {
        question: question.to_string(),
        reply,
    };
    if sender.send(request).is_err() {
        return Ok(None);
    }
    match tokio::time::timeout_at(deadline, answer).await {
        Err(_) => Err(SideQuestionFailure::Failed(
            crate::runtime::claude_code::side_question_timeout_message(),
        )),
        Ok(Err(_process_ended)) => Ok(None),
        Ok(Ok(answer)) => answer.map(Some).map_err(SideQuestionFailure::Failed),
    }
}

/// What a cold side-question process needs: the thread's session and the
/// flags its main session runs with.
struct ColdSession {
    cwd: PathBuf,
    session_id: String,
    config_dir: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    allowed_tools: String,
    user_env: Vec<(String, String)>,
    binary_override: Option<String>,
    permission_mode: Option<String>,
}

/// Whether a side question with this id was already asked on the thread.
async fn was_asked(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    side_question_id: Uuid,
) -> Result<bool, SideQuestionFailure> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM events
             WHERE aggregate = 'thread'
               AND aggregate_id = $1
               AND event_type = 'SideQuestionAsked'
               AND payload->>'side_question_id' = $2
        )",
    )
    .bind(thread_id.to_string())
    .bind(side_question_id.to_string())
    .fetch_one(pool)
    .await
    .map_err(|e| SideQuestionFailure::Failed(format!("Could not read the thread: {e}")))
}

/// Record one side-question event on its thread.
async fn record(
    bus: &EventBus,
    thread_id: Uuid,
    event: ThreadEvent,
    meta: EventMeta,
) -> Result<(), SideQuestionFailure> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta,
    })
    .await
    .map(|_| ())
    .map_err(|e| SideQuestionFailure::Failed(format!("Could not record the side question: {e}")))
}

/// The event that settles an ask, from how it ended.
fn settled_event(
    side_question_id: Uuid,
    outcome: &Result<String, SideQuestionFailure>,
) -> ThreadEvent {
    match outcome {
        Ok(answer) => ThreadEvent::SideQuestionAnswered {
            side_question_id,
            answer: answer.clone(),
        },
        Err(failure) => ThreadEvent::SideQuestionFailed {
            side_question_id,
            error: match failure {
                SideQuestionFailure::Refused(text) => text.to_string(),
                SideQuestionFailure::Failed(text) => text.clone(),
                SideQuestionFailure::AlreadyAsked | SideQuestionFailure::NotAsked => {
                    "The side question could not be asked.".to_string()
                }
            },
        },
    }
}

/// Record a failure for every ask no answer or failure settled. Run once at
/// startup, since the process that would have settled them is gone.
pub(crate) async fn fail_unsettled_side_questions(
    pool: &sqlx::PgPool,
    bus: &EventBus,
) -> Result<usize, sqlx::Error> {
    let unsettled: Vec<(String, String)> = sqlx::query_as(
        "SELECT a.aggregate_id, a.payload->>'side_question_id'
           FROM events a
          WHERE a.aggregate = 'thread'
            AND a.event_type = 'SideQuestionAsked'
            AND NOT EXISTS (
                SELECT 1 FROM events s
                 WHERE s.aggregate = 'thread'
                   AND s.aggregate_id = a.aggregate_id
                   AND s.event_type IN ('SideQuestionAnswered', 'SideQuestionFailed')
                   AND s.payload->>'side_question_id' = a.payload->>'side_question_id'
            )",
    )
    .fetch_all(pool)
    .await?;
    let mut failed = 0;
    for (thread_id, side_question_id) in unsettled {
        let (Ok(thread_id), Ok(side_question_id)) = (
            Uuid::parse_str(&thread_id),
            Uuid::parse_str(&side_question_id),
        ) else {
            crate::log!(
                "[SideQuestion] Skipping an unsettled side question with a malformed id: thread {thread_id:?}, id {side_question_id:?}"
            );
            continue;
        };
        let event = ThreadEvent::SideQuestionFailed {
            side_question_id,
            error: INTERRUPTED_BY_RESTART.to_string(),
        };
        match record(bus, thread_id, event, EventMeta::NONE).await {
            Ok(()) => failed += 1,
            Err(e) => crate::log!(
                "[SideQuestion] Could not fail interrupted side question {side_question_id} on thread {thread_id}: {e:?}"
            ),
        }
    }
    Ok(failed)
}

impl LucidosEngine {
    /// Ask a side question and record it: the ask, then its answer or
    /// failure. A refused thread, an empty question or a repeated id records
    /// nothing, since no card is owed for them.
    pub(crate) async fn ask_side_question(
        &self,
        thread_id: Uuid,
        side_question_id: Uuid,
        question: &str,
        actor: MessageOrigin,
    ) -> Result<String, SideQuestionFailure> {
        let question = question.trim();
        if question.is_empty() {
            return Err(SideQuestionFailure::Refused(EMPTY_QUESTION));
        }
        check_thread(self.pool(), thread_id).await?;
        if !self.has_session(thread_id).await? {
            return Err(SideQuestionFailure::Refused(NO_SESSION_YET));
        }
        {
            // Held from the check to the record, so two requests with one id
            // cannot both pass `was_asked`.
            let _admission = ASK_ADMISSION.lock().await;
            if was_asked(self.pool(), thread_id, side_question_id).await? {
                return Err(SideQuestionFailure::AlreadyAsked);
            }
            let asked = ThreadEvent::SideQuestionAsked {
                side_question_id,
                question: question.to_string(),
            };
            record(
                &self.event_bus,
                thread_id,
                asked,
                EventMeta::with_actor(Some(actor)),
            )
            .await?;
        }
        let outcome = self.answer_side_question(thread_id, question).await;
        let settled = settled_event(side_question_id, &outcome);
        record(&self.event_bus, thread_id, settled, EventMeta::NONE).await?;
        outcome
    }

    /// Whether the thread has a Claude Code session to ask: a live process,
    /// or a session a cold process can resume.
    async fn has_session(&self, thread_id: Uuid) -> Result<bool, SideQuestionFailure> {
        let live = self
            .agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(|session| !session.process_exited && session.side_question_tx.is_some());
        if live {
            return Ok(true);
        }
        let pool = self.pool();
        let config_dir = super::lookup_pinned_cc_config_dir(pool, thread_id)
            .await
            .map_err(|e| SideQuestionFailure::Failed(format!("Could not read the thread: {e}")))?;
        Ok(
            super::resume::resume_sid_for_account(pool, thread_id, config_dir.as_deref())
                .await
                .is_some(),
        )
    }

    /// Startup recovery: fail every ask the previous process left unanswered,
    /// so no card waits forever.
    pub async fn recover_unsettled_side_questions(&self) {
        match fail_unsettled_side_questions(self.pool(), &self.event_bus).await {
            Ok(0) => {}
            Ok(failed) => log!(
                "[SideQuestion] Failed {} side questions a restart interrupted",
                failed
            ),
            Err(e) => log!(
                "[SideQuestion] Could not recover unsettled side questions: {}",
                e
            ),
        }
    }

    /// Record that the user dismissed a side question's card.
    pub(crate) async fn dismiss_side_question(
        &self,
        thread_id: Uuid,
        side_question_id: Uuid,
        actor: MessageOrigin,
    ) -> Result<(), SideQuestionFailure> {
        if !was_asked(self.pool(), thread_id, side_question_id).await? {
            return Err(SideQuestionFailure::NotAsked);
        }
        let dismissed = ThreadEvent::SideQuestionDismissed { side_question_id };
        record(
            &self.event_bus,
            thread_id,
            dismissed,
            EventMeta::with_actor(Some(actor)),
        )
        .await
    }

    /// Answer a side question in a Claude Code thread, from its live process
    /// when it has one and from a short-lived resumed process otherwise.
    async fn answer_side_question(
        &self,
        thread_id: Uuid,
        question: &str,
    ) -> Result<String, SideQuestionFailure> {
        // One budget for both paths, so a cold retry cannot outlast the browser's wait.
        let deadline =
            tokio::time::Instant::now() + crate::runtime::claude_code::SIDE_QUESTION_TIMEOUT;
        if let Some(answer) = ask_live(&self.agent_sessions, thread_id, question, deadline).await? {
            return Ok(answer);
        }
        let cold = self.cold_session(thread_id).await?;
        let args = crate::runtime::SpawnArgs {
            worktree_path: &cold.cwd,
            coding_agent_kind: Default::default(),
            workspace_path: self.workspace_path(),
            allowed_tools: Some(&cold.allowed_tools),
            system_prompt: None,
            resume_session_id: Some(&cold.session_id),
            model: cold.model.as_deref(),
            reasoning_effort: cold.effort.as_deref(),
            thread_id,
            spawning_event_id: None,
            repo_name: None,
            interactive: false,
            user_env_vars: &cold.user_env,
            claude_config_dir: cold.config_dir.as_deref(),
            binary_override: cold.binary_override.as_deref(),
            permission_mode: cold.permission_mode.as_deref(),
        };
        crate::runtime::claude_code::ask_side_question_cold(args, question, deadline)
            .await
            .map_err(|e| SideQuestionFailure::Failed(e.to_string()))
    }

    /// Resolve an idle thread's session the way a follow-up spawn would.
    ///
    /// The cwd is the thread's worktree while it exists. Claude Code finds a
    /// session by id from any cwd, so a cleaned-up worktree falls back to the
    /// workspace.
    async fn cold_session(&self, thread_id: Uuid) -> Result<ColdSession, SideQuestionFailure> {
        let pool = self.pool();
        let config_dir = super::lookup_pinned_cc_config_dir(pool, thread_id)
            .await
            .map_err(|e| SideQuestionFailure::Failed(format!("Could not read the thread: {e}")))?;
        let session_id =
            super::resume::resume_sid_for_account(pool, thread_id, config_dir.as_deref())
                .await
                .ok_or(SideQuestionFailure::Refused(NO_SESSION_YET))?;
        let worktree = super::resume::lookup_latest_worktree_path(pool, thread_id).await;
        let cwd = match worktree {
            Some(path) if tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_dir()) => path,
            _ => self.workspace_path().to_path_buf(),
        };
        let (model, effort) = self.cc_thread_settings(thread_id).await;
        let preference = |key: &'static str| async move {
            crate::core::PreferenceStore::get(pool, key)
                .await
                .unwrap_or_else(|e| {
                    log!("[SideQuestion] Failed to load {} preference: {}", key, e);
                    None
                })
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Ok(ColdSession {
            cwd,
            session_id,
            config_dir,
            model,
            effort,
            allowed_tools: crate::engine::claude_code::cc_allowed_tools(&self.grants_dir()),
            user_env: crate::core::EnvironmentVariableStore::spawn_pairs(pool, "SideQuestion")
                .await,
            binary_override: preference(crate::core::PREF_CODING_AGENT_CLAUDE_PATH).await,
            permission_mode: preference(crate::core::PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE)
                .await,
        })
    }
}

#[cfg(test)]
#[path = "side_question_tests.rs"]
mod tests;
