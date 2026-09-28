//! Side questions (`/btw`) in Claude Code threads: a quick question answered
//! from the session's context, beside any running turn, with no tools.
//!
//! Nothing here emits an event or writes to the thread. The answer goes back to
//! the asker and nowhere else, so it never enters a context builder
//! (ADR 0318).

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::engine::types::AgentSession;
use crate::engine::LucidosEngine;
use crate::runtime::{CodingAgent, SideQuestionRequest};

/// Why a side question got no answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SideQuestionFailure {
    /// The thread cannot take side questions. The text tells the user why.
    Refused(&'static str),
    /// Claude Code could not answer. The text says what went wrong.
    Failed(String),
}

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

impl LucidosEngine {
    /// Answer a side question in a Claude Code thread, from its live process
    /// when it has one and from a short-lived resumed process otherwise.
    pub(crate) async fn ask_side_question(
        &self,
        thread_id: Uuid,
        question: &str,
    ) -> Result<String, SideQuestionFailure> {
        let question = question.trim();
        if question.is_empty() {
            return Err(SideQuestionFailure::Refused(EMPTY_QUESTION));
        }
        check_thread(self.pool(), thread_id).await?;
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
