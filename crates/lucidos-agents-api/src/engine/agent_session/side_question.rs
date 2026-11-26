use crate::engine::chat::process::side_question::ProcessSideQuestionExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::session_copy::SessionCopy;
pub(crate) use crate::engine::session_copy::SideQuestionFailure;
use crate::engine::side_question_policy::SIDE_QUESTION_TIMEOUT;
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::types::ChatImage;
use crate::engine::LucidosEngine;
use crate::runtime::CodingAgent;

pub(crate) const INTERRUPTED_BY_RESTART: &str = "Interrupted by a restart. Ask again.";

pub(crate) const EMPTY_QUESTION: &str = "Type the side question first.";
pub(crate) const NO_SUCH_THREAD: &str = "Side questions work once this thread has started.";
const NO_SESSION_YET: &str =
    "This thread has no coding-agent session yet. Wait for its first reply, then ask again.";

const REMEMBERED_CC_PROMPTS: usize = 128;

/// INVARIANT: Serializes the duplicate-id check with the ask's record. Held only for
/// those two quick writes, never across the answer.
static ASK_ADMISSION: Mutex<()> = Mutex::const_new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SideQuestionAgent {
    ClaudeCode,
    Codex,
    Lucidos,
}

type ThreadRow = (String, Option<String>, String);

fn agent_for(row: Option<ThreadRow>) -> Result<SideQuestionAgent, &'static str> {
    match row {
        None => Err(NO_SUCH_THREAD),
        Some((_, _, state)) if state == "composing" => Err(NO_SUCH_THREAD),
        Some((source, coding_agent, _)) if source == "claude_code" => {
            match CodingAgent::parse(coding_agent.as_deref().unwrap_or_default()) {
                CodingAgent::ClaudeCode => Ok(SideQuestionAgent::ClaudeCode),
                CodingAgent::Codex => Ok(SideQuestionAgent::Codex),
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

/// INVARIANT: Record a failure for every ask no answer or failure settled. Run once at
/// startup, since the process that would have settled them is gone.
pub(crate) async fn fail_unsettled_side_questions(
    pool: &sqlx::PgPool,
    bus: &EventBus,
) -> Result<usize, sqlx::Error> {
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
            crate::log_legacy!(
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
            Err(e) => crate::log_legacy!(
                "[SideQuestion] Could not fail interrupted side question {side_question_id} on thread {thread_id}: {e:?}"
            ),
        }
    }
    Ok(failed)
}

pub async fn recover_unsettled_side_questions(engine: &LucidosEngine) {
    engine.recover_unsettled_side_questions().await;
}

#[easy_ext::ext(AgentSessionSideQuestionExt)]
pub(crate) impl LucidosEngine {
    async fn ask_side_question(
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
        if agent != SideQuestionAgent::Lucidos && !self.has_session(thread_id, agent).await? {
            return Err(SideQuestionFailure::Refused(NO_SESSION_YET));
        }
        {
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
        let outcome = if agent == SideQuestionAgent::Codex {
            self.answer_codex_side_question(thread_id, question, image_hashes, deadline)
                .await
        } else {
            match load_images(self.workspace_path(), image_hashes) {
                Err(failure) => Err(failure),
                Ok(images) => {
                    if agent == SideQuestionAgent::ClaudeCode {
                        self.answer_claude_code_side_question(
                            thread_id, question, &images, deadline,
                        )
                        .await
                    } else {
                        self.answer_lucidos_side_question(thread_id, question, &images, deadline)
                            .await
                            .map_err(SideQuestionFailure::Failed)
                    }
                }
            }
        };
        let settled = settled_event(side_question_id, &outcome);
        record(&self.event_bus, thread_id, settled, EventMeta::NONE).await?;
        outcome
    }

    async fn has_session(
        &self,
        thread_id: Uuid,
        agent: SideQuestionAgent,
    ) -> Result<bool, SideQuestionFailure> {
        if agent == SideQuestionAgent::Codex {
            return Ok(super::resume::lookup_latest_session_id_for_agent(
                self.pool(),
                thread_id,
                CodingAgent::Codex,
            )
            .await
            .is_some());
        }
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

    fn remember_cc_system_prompt(&self, thread_id: Uuid, prompt: &str) {
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

    async fn recover_unsettled_side_questions(&self) {
        match fail_unsettled_side_questions(self.pool(), &self.event_bus).await {
            Ok(0) => {}
            Ok(failed) => log_legacy!(
                "[SideQuestion] Failed {} side questions a restart interrupted",
                failed
            ),
            Err(e) => log_legacy!(
                "[SideQuestion] Could not recover unsettled side questions: {}",
                e
            ),
        }
    }

    async fn dismiss_side_question(
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

    async fn answer_claude_code_side_question(
        &self,
        thread_id: Uuid,
        question: &str,
        images: &[ChatImage],
        deadline: tokio::time::Instant,
    ) -> Result<String, SideQuestionFailure> {
        let copy = self.session_copy(thread_id).await?;
        let args = copy.spawn_args(self.workspace_path(), thread_id);
        let reply =
            crate::runtime::claude_code::ask_side_question(args, question, images, deadline)
                .await
                .map_err(|e| SideQuestionFailure::Failed(e.to_string()))?;
        crate::engine::AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::SideQuestion,
        )
        .record_usage(
            &copy.model_of(&reply),
            question.chars().count(),
            reply.usage,
        )
        .await;
        reply.answer.map_err(SideQuestionFailure::Failed)
    }

    async fn answer_codex_side_question(
        &self,
        thread_id: Uuid,
        question: &str,
        hashes: &[String],
        deadline: tokio::time::Instant,
    ) -> Result<String, SideQuestionFailure> {
        let copy = self.session_copy_for(thread_id, CodingAgent::Codex).await?;
        let paths = hashes
            .iter()
            .map(|hash| {
                crate::core::blobs::resolve_blob(self.workspace_path(), hash)
                    .map(|blob| blob.path)
                    .ok_or_else(|| {
                        SideQuestionFailure::Failed(format!(
                            "Image {hash} was deleted before it was read"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let reply = crate::runtime::codex_side_question::ask_side_question(
            copy.spawn_args(self.workspace_path(), thread_id),
            question,
            &paths,
            deadline,
        )
        .await
        .map_err(|e| SideQuestionFailure::Failed(e.to_string()))?;
        crate::engine::AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::SideQuestion,
        )
        .record_usage(
            &copy.model_of(&reply),
            question.chars().count(),
            reply.usage,
        )
        .await;
        reply.answer.map_err(SideQuestionFailure::Failed)
    }

    async fn session_copy(&self, thread_id: Uuid) -> Result<SessionCopy, SideQuestionFailure> {
        self.session_copy_for(thread_id, CodingAgent::ClaudeCode)
            .await
    }

    async fn session_copy_for(
        &self,
        thread_id: Uuid,
        coding_agent: CodingAgent,
    ) -> Result<SessionCopy, SideQuestionFailure> {
        let pool = self.pool();
        let recorded_pin = if coding_agent == CodingAgent::ClaudeCode {
            super::lookup_pinned_cc_config_dir(pool, thread_id)
                .await
                .map_err(|e| {
                    SideQuestionFailure::Failed(format!("Could not read the thread: {e}"))
                })?
        } else {
            None
        };
        let pinned_dir = recorded_pin.as_ref().map(|pin| pin.dir.as_str());
        let session_id = match coding_agent {
            CodingAgent::ClaudeCode => {
                super::resume::resume_sid_for_account(pool, thread_id, pinned_dir).await
            }
            CodingAgent::Codex => {
                super::resume::lookup_latest_session_id_for_agent(pool, thread_id, coding_agent)
                    .await
            }
        }
        .ok_or(SideQuestionFailure::Refused(NO_SESSION_YET))?;
        let worktree = super::resume::lookup_latest_worktree_path(pool, thread_id).await;
        let cwd = match worktree {
            Some(path) if tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_dir()) => path,
            _ => self.workspace_path().to_path_buf(),
        };
        let (model, effort) = self.cc_thread_settings_on(thread_id, coding_agent).await;
        let system_prompt = self
            .cc_system_prompts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&thread_id)
            .cloned();
        let user_env =
            crate::core::EnvironmentVariableStore::spawn_pairs(pool, "SideQuestion").await;
        // INVARIANT: Computed from the same cwd as the session's own spawn, so both name
        // the same directories and share one prompt prefix.
        let additional_directories =
            crate::engine::repo_directory_grants::resolve(&cwd, self.workspace_path()).await;
        let sandbox = crate::runtime::agent_sandbox::Sandbox::resolve(pool, self.workspace_path())
            .await
            .map_err(|e| SideQuestionFailure::Failed(e.to_string()))?;
        let build_slot_class = crate::engine::main_check::repair_build_class(pool, thread_id)
            .await
            .map_err(|error| {
                SideQuestionFailure::Failed(format!("Could not read repair build class: {error}"))
            })?;
        Ok(SessionCopy {
            build_slot_class,
            cwd,
            session_id,
            system_prompt,
            account_pin: recorded_pin.map(|pin| pin.resolve(&user_env)),
            model,
            effort,
            allowed_tools: crate::engine::claude_code::cc_allowed_tools(&self.event_bus).await,
            user_env,
            binary_override: match coding_agent {
                CodingAgent::ClaudeCode => {
                    crate::core::prefs::CODING_AGENT_CLAUDE_PATH
                        .read(pool)
                        .await
                }
                CodingAgent::Codex => crate::core::prefs::CODING_AGENT_CODEX_PATH.read(pool).await,
            },
            permission_mode: crate::core::prefs::CODING_AGENT_CLAUDE_PERMISSION_MODE
                .stored(pool)
                .await,
            additional_directories,
            sandbox,
        })
    }
}

#[cfg(test)]
#[path = "side_question_tests.rs"]
mod tests;
