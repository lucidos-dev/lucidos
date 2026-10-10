//! Side questions: a quick question answered from a thread's context,
//! beside any running turn, with no tools. A Claude Code thread asks a copy of
//! its session. A Lucidos Agent thread asks its own model once
//! (`chat::process::side_question`).
//!
//! Each ask is recorded as side-question thread events, so its card survives a
//! reload and shows on every device. No agent ever reads them: every generic
//! event reader excludes them (ADR 0320).

use std::path::PathBuf;

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::api::ChatImage;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::runtime::CodingAgent;

/// Why a side question got no answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SideQuestionFailure {
    /// The thread cannot take side questions. The text tells the user why.
    Refused(&'static str),
    /// The ask names an image this workspace never received.
    UnknownImage(String),
    /// A side question with this id is running or was answered. Only a
    /// failed one may be asked again.
    AlreadyAsked,
    /// Nothing asked with this id on the thread, so there is nothing to dismiss.
    NotAsked,
    /// Claude Code could not answer. The text says what went wrong.
    Failed(String),
}

/// How long a side question may take, on either agent. A Claude Code copy
/// resumes the whole transcript before its model call starts.
pub(crate) const SIDE_QUESTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Model turns a side question may take, on either agent. A refused tool call
/// costs one, so this leaves room to answer after a stray attempt.
pub(crate) const SIDE_QUESTION_MAX_TURNS: usize = 3;

/// What the model reads before the question, on either agent. The Claude Code
/// copy resumes a saved transcript, whose running step reads as interrupted.
pub(crate) const SIDE_QUESTION_INSTRUCTIONS: &str = "This is a side question from the user, \
asked beside the main conversation. Answer it directly in one reply, from what you already \
know. You cannot use tools here: every tool call is refused. Do not continue the main task \
and do not promise any action. If the answer needs a tool, say so. Then suggest asking in the \
main conversation. The main task may still be running: a last step that reads as interrupted \
was not stopped.";

/// What the model reads when it calls a tool anyway, on either agent.
pub(crate) const SIDE_QUESTION_TOOL_REFUSAL: &str =
    "Side questions cannot use tools. Answer from what you already know.";

/// What the asker reads when every turn reached for a tool.
pub(crate) fn kept_reaching_for_tools(agent: &str) -> String {
    format!("{agent} kept reaching for tools instead of answering. Ask it in the main conversation instead.")
}

/// What the asker reads when a side question runs past its deadline.
pub(crate) fn side_question_timeout_message(agent: &str) -> String {
    format!(
        "{agent} did not answer within {} seconds",
        SIDE_QUESTION_TIMEOUT.as_secs()
    )
}

/// What startup recovery records for an ask a restart left unanswered.
pub(crate) const INTERRUPTED_BY_RESTART: &str = "Interrupted by a restart. Ask again.";

pub(crate) const EMPTY_QUESTION: &str = "Type the side question first.";
pub(crate) const NO_SUCH_THREAD: &str = "Side questions work once this thread has started.";
pub(crate) const CODEX_UNSUPPORTED: &str =
    "Side questions are not available in Codex threads. Send it as a normal message instead.";
const NO_SESSION_YET: &str =
    "This thread has no Claude Code session yet. Wait for its first reply, then ask again.";

/// How many Claude Code spawn prompts the engine remembers. Each is a whole
/// appended system prompt, so the cap bounds memory on a long-running engine.
const REMEMBERED_CC_PROMPTS: usize = 128;

/// Serializes the duplicate-id check with the ask's record. Held only for
/// those two quick writes, never across the answer.
static ASK_ADMISSION: Mutex<()> = Mutex::const_new(());

/// Which agent answers a thread's side questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SideQuestionAgent {
    ClaudeCode,
    Lucidos,
}

/// A thread's `thread_summaries` row: `(source, coding_agent, state)`.
type ThreadRow = (String, Option<String>, String);

/// The agent for a thread, from its row, or the refusal when it takes no side
/// question. A draft still composing has not started, so it takes none.
fn agent_for(row: Option<ThreadRow>) -> Result<SideQuestionAgent, &'static str> {
    match row {
        None => Err(NO_SUCH_THREAD),
        Some((_, _, state)) if state == "composing" => Err(NO_SUCH_THREAD),
        Some((source, coding_agent, _)) if source == "claude_code" => {
            match CodingAgent::parse(coding_agent.as_deref().unwrap_or_default()) {
                CodingAgent::ClaudeCode => Ok(SideQuestionAgent::ClaudeCode),
                CodingAgent::Codex => Err(CODEX_UNSUPPORTED),
            }
        }
        Some(_) => Ok(SideQuestionAgent::Lucidos),
    }
}

async fn check_thread(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<SideQuestionAgent, SideQuestionFailure> {
    let row: Option<ThreadRow> = sqlx::query_as(
        "SELECT source, coding_agent, state FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| SideQuestionFailure::Failed(format!("Could not read the thread: {e}")))?;
    agent_for(row).map_err(SideQuestionFailure::Refused)
}

/// Refuse an ask naming an image this workspace never received. Only a stat
/// per image, so a refused ask never reads the bytes.
fn check_images(workspace: &std::path::Path, hashes: &[String]) -> Result<(), SideQuestionFailure> {
    match hashes
        .iter()
        .find(|hash| crate::core::blobs::resolve_blob(workspace, hash).is_none())
    {
        Some(hash) => Err(SideQuestionFailure::UnknownImage(format!(
            "The side question names image {hash}, which was never uploaded to this workspace"
        ))),
        None => Ok(()),
    }
}

/// Read every attached image from the blob store. `check_images` has already
/// found each one, so a miss means it was deleted since.
fn load_images(
    workspace: &std::path::Path,
    hashes: &[String],
) -> Result<Vec<ChatImage>, SideQuestionFailure> {
    hashes
        .iter()
        .map(|hash| {
            crate::core::blobs::read_blob_as_base64(workspace, hash)
                .map(|(base64, mime_type)| ChatImage { base64, mime_type })
                .ok_or_else(|| {
                    SideQuestionFailure::Failed(format!(
                        "Image {hash} was deleted before it was read"
                    ))
                })
        })
        .collect()
}

/// What a side question's session copy needs: the thread's session and the
/// flags its main session runs with.
struct SessionCopy {
    cwd: PathBuf,
    session_id: String,
    /// What the session's latest spawn appended, when this engine saw it.
    system_prompt: Option<String>,
    account_pin: Option<crate::runtime::AccountPin>,
    model: Option<String>,
    effort: Option<String>,
    allowed_tools: String,
    user_env: Vec<(String, String)>,
    binary_override: Option<String>,
    permission_mode: Option<String>,
    additional_directories: Vec<PathBuf>,
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

/// Whether an ask under this id may run: never asked, or every ask failed and
/// none was answered. A retry re-asks under the card's own id, so the card
/// keeps its place. One still running stays refused.
async fn may_ask(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    side_question_id: Uuid,
) -> Result<bool, SideQuestionFailure> {
    sqlx::query_scalar::<_, bool>(
        "SELECT COUNT(*) FILTER (WHERE event_type = 'SideQuestionAsked')
              = COUNT(*) FILTER (WHERE event_type = 'SideQuestionFailed')
            AND COUNT(*) FILTER (WHERE event_type = 'SideQuestionAnswered') = 0
           FROM events
          WHERE aggregate = 'thread'
            AND aggregate_id = $1
            AND event_type IN ('SideQuestionAsked', 'SideQuestionAnswered', 'SideQuestionFailed')
            AND payload->>'side_question_id' = $2",
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
                SideQuestionFailure::Failed(text) | SideQuestionFailure::UnknownImage(text) => {
                    text.clone()
                }
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
    // Counted, not matched: a retry asks again under an id whose first ask
    // already failed, so one settlement does not settle every ask.
    let unsettled: Vec<(String, String)> = sqlx::query_as(
        "SELECT aggregate_id, payload->>'side_question_id'
           FROM events
          WHERE aggregate = 'thread'
            AND event_type IN ('SideQuestionAsked', 'SideQuestionAnswered', 'SideQuestionFailed')
          GROUP BY aggregate_id, payload->>'side_question_id'
         HAVING COUNT(*) FILTER (WHERE event_type = 'SideQuestionAsked')
              > COUNT(*) FILTER (WHERE event_type <> 'SideQuestionAsked')",
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
    /// failure. A refused thread, an empty question, an unknown image or an id
    /// still running or answered records nothing, since no card is owed.
    pub(crate) async fn ask_side_question(
        &self,
        thread_id: Uuid,
        side_question_id: Uuid,
        question: &str,
        image_hashes: &[String],
        actor: MessageOrigin,
    ) -> Result<String, SideQuestionFailure> {
        let question = question.trim();
        if question.is_empty() {
            return Err(SideQuestionFailure::Refused(EMPTY_QUESTION));
        }
        let agent = check_thread(self.pool(), thread_id).await?;
        check_images(self.workspace_path(), image_hashes)?;
        if agent == SideQuestionAgent::ClaudeCode && !self.has_session(thread_id).await? {
            return Err(SideQuestionFailure::Refused(NO_SESSION_YET));
        }
        {
            // Held from the check to the record, so two requests with one id
            // cannot both pass `may_ask`.
            let _admission = ASK_ADMISSION.lock().await;
            if !may_ask(self.pool(), thread_id, side_question_id).await? {
                return Err(SideQuestionFailure::AlreadyAsked);
            }
            let asked = ThreadEvent::SideQuestionAsked {
                side_question_id,
                question: question.to_string(),
                image_hashes: image_hashes.to_vec(),
            };
            record(
                &self.event_bus,
                thread_id,
                asked,
                EventMeta::with_actor(Some(actor)),
            )
            .await?;
        }
        let deadline = tokio::time::Instant::now() + SIDE_QUESTION_TIMEOUT;
        let outcome = match load_images(self.workspace_path(), image_hashes) {
            Err(failure) => Err(failure),
            Ok(images) => match agent {
                SideQuestionAgent::ClaudeCode => {
                    self.answer_claude_code_side_question(thread_id, question, &images, deadline)
                        .await
                }
                SideQuestionAgent::Lucidos => self
                    .answer_lucidos_side_question(thread_id, question, &images, deadline)
                    .await
                    .map_err(SideQuestionFailure::Failed),
            },
        };
        let settled = settled_event(side_question_id, &outcome);
        record(&self.event_bus, thread_id, settled, EventMeta::NONE).await?;
        outcome
    }

    /// Whether the thread has a saved Claude Code session a copy can resume.
    async fn has_session(&self, thread_id: Uuid) -> Result<bool, SideQuestionFailure> {
        let pool = self.pool();
        let recorded_pin = super::lookup_pinned_cc_config_dir(pool, thread_id)
            .await
            .map_err(|e| SideQuestionFailure::Failed(format!("Could not read the thread: {e}")))?;
        let pinned_dir = recorded_pin.as_ref().map(|pin| pin.dir.as_str());
        Ok(
            super::resume::resume_sid_for_account(pool, thread_id, pinned_dir)
                .await
                .is_some(),
        )
    }

    /// Record the system prompt a Claude Code spawn appended, for the thread's
    /// side questions. At most `REMEMBERED_CC_PROMPTS` are kept. An evicted
    /// thread asks uncached and without that prompt until its next spawn.
    pub(crate) fn remember_cc_system_prompt(&self, thread_id: Uuid, prompt: &str) {
        let mut prompts = self
            .cc_system_prompts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if prompts.len() >= REMEMBERED_CC_PROMPTS && !prompts.contains_key(&thread_id) {
            if let Some(evicted) = prompts.keys().next().copied() {
                prompts.remove(&evicted);
            }
        }
        prompts.insert(thread_id, prompt.to_string());
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

    /// Answer a side question in a Claude Code thread, from a copy of its
    /// session that shares the session's prompt prefix.
    async fn answer_claude_code_side_question(
        &self,
        thread_id: Uuid,
        question: &str,
        images: &[ChatImage],
        deadline: tokio::time::Instant,
    ) -> Result<String, SideQuestionFailure> {
        let copy = self.session_copy(thread_id).await?;
        let args = crate::runtime::SpawnArgs {
            worktree_path: &copy.cwd,
            coding_agent_kind: Default::default(),
            workspace_path: self.workspace_path(),
            allowed_tools: Some(&copy.allowed_tools),
            system_prompt: copy.system_prompt.as_deref(),
            resume_session_id: Some(&copy.session_id),
            model: copy.model.as_deref(),
            reasoning_effort: copy.effort.as_deref(),
            thread_id,
            spawning_event_id: None,
            repo_name: None,
            interactive: false,
            user_env_vars: &copy.user_env,
            account_pin: copy.account_pin.as_ref(),
            binary_override: copy.binary_override.as_deref(),
            permission_mode: copy.permission_mode.as_deref(),
            additional_directories: &copy.additional_directories,
        };
        let reply =
            crate::runtime::claude_code::ask_side_question(args, question, images, deadline)
                .await
                .map_err(|e| SideQuestionFailure::Failed(e.to_string()))?;
        // The copy is a real model call, so its cost is recorded like any other.
        let model = reply
            .model
            .or(copy.model)
            .unwrap_or_else(|| "unknown".to_string());
        crate::engine::AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::SideQuestion,
        )
        .record_usage(&model, question.chars().count(), reply.usage)
        .await;
        reply.answer.map_err(SideQuestionFailure::Failed)
    }

    /// Resolve the thread's session the way a follow-up spawn would.
    ///
    /// The cwd is the thread's worktree while it exists. Claude Code finds a
    /// session by id from any cwd, so a cleaned-up worktree falls back to the
    /// workspace.
    async fn session_copy(&self, thread_id: Uuid) -> Result<SessionCopy, SideQuestionFailure> {
        let pool = self.pool();
        let recorded_pin = super::lookup_pinned_cc_config_dir(pool, thread_id)
            .await
            .map_err(|e| SideQuestionFailure::Failed(format!("Could not read the thread: {e}")))?;
        let pinned_dir = recorded_pin.as_ref().map(|pin| pin.dir.as_str());
        let session_id = super::resume::resume_sid_for_account(pool, thread_id, pinned_dir)
            .await
            .ok_or(SideQuestionFailure::Refused(NO_SESSION_YET))?;
        let worktree = super::resume::lookup_latest_worktree_path(pool, thread_id).await;
        let cwd = match worktree {
            Some(path) if tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_dir()) => path,
            _ => self.workspace_path().to_path_buf(),
        };
        let (model, effort) = self.cc_thread_settings(thread_id).await;
        let system_prompt = self
            .cc_system_prompts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&thread_id)
            .cloned();
        let user_env =
            crate::core::EnvironmentVariableStore::spawn_pairs(pool, "SideQuestion").await;
        // Computed from the same cwd as the session's own spawn, so both name
        // the same directories and share one prompt prefix.
        let additional_directories =
            crate::engine::repo_directory_grants::resolve(&cwd, self.workspace_path()).await;
        Ok(SessionCopy {
            cwd,
            session_id,
            system_prompt,
            account_pin: recorded_pin.map(|pin| pin.resolve(&user_env)),
            model,
            effort,
            allowed_tools: crate::engine::claude_code::cc_allowed_tools(&self.grants_dir()),
            user_env,
            binary_override: crate::core::prefs::CODING_AGENT_CLAUDE_PATH
                .read(pool)
                .await,
            permission_mode: crate::core::prefs::CODING_AGENT_CLAUDE_PERMISSION_MODE
                .stored(pool)
                .await,
            additional_directories,
        })
    }
}

#[cfg(test)]
#[path = "side_question_tests.rs"]
mod tests;
