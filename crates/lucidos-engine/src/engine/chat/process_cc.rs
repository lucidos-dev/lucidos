//! Direct Claude Code mode: the branch that runs a coding agent against the
//! current thread. The caller claimed the thread at its fast path, so a
//! follow-up sent during startup queues for this run and a Stop cancels it.
//! The caller's `ThreadGuard` stays alive across the await: the run's
//! `chat_cancel` arm listens on its token for a request abort or a shutdown.

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::agent_session::spawn_claim::SpawnClaim;
use crate::engine::types::ProcessResult;
use crate::engine::LucidosEngine;

impl LucidosEngine {
    /// Spawn or resume a coding-agent turn for the current thread on the claim
    /// the caller took. Returns whatever `run_direct_agent` returns.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_cc_chat_branch(
        &self,
        mut claim: SpawnClaim,
        request_id: Uuid,
        user_message: &str,
        user_images: Option<&[crate::api::ChatImage]>,
        repo_id: Option<&str>,
        cc_model: Option<&str>,
        reasoning_effort: Option<&str>,
        conflict_change_id: Option<Uuid>,
        origin_id: Uuid,
        spawning_event_id: Option<Uuid>,
        cancel_token: &CancellationToken,
        chat_start: std::time::Instant,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let thread_id = claim.thread_id;
        log!(
            "[Chat] [TIMING] pre-CC overhead: {:?}",
            chat_start.elapsed()
        );
        // A merge-conflict turn holds the change's apply claim for its run.
        let conflict_claim = match conflict_change_id {
            Some(change_id) => Some(
                self.claim_pending_change(change_id)
                    .await?
                    .ok_or(crate::engine::change_ops::apply_claim::APPLY_IN_PROGRESS_MESSAGE)?,
            ),
            None => None,
        };
        let repo_id_str = repo_id.map(|s| s.to_string());
        let cc_model_str = cc_model.map(|s| s.to_string());
        let cc_effort_str = reasoning_effort.map(|s| s.to_string());

        // Recover the prior Claude Code session id from the event store so a follow-up
        // after the in-memory entry was removed (subprocess died, engine
        // restart, change-less idle) still resumes the same conversation.
        // Without this, the resolver's auto-detect path returns no sid when
        // the most recent lifecycle event is SessionEnded — even though
        // CodingAgentIdled with a usable sid exists earlier in the timeline.
        let resume_sid =
            crate::engine::agent_session::lookup_latest_cc_session_id(&self.pool, thread_id).await;
        // Direct run_direct_agent here is NOT a parallel path:
        // `run_cc_chat_branch` IS the CC slow path of
        // `process_message_with_steps_internal` (chat/process/run.rs's
        // `use_coding_agent == Some(true)` branch routes here). The
        // unified router funnels every chat-driven and agent-driven CC
        // turn through this call; it's the bottom of the funnel.
        let result = self
            .run_direct_agent(
                &mut claim,
                request_id,
                user_message,
                user_images,
                origin_id,
                &[],
                spawning_event_id,
                cancel_token,
                conflict_claim.as_ref(),
                None,
                repo_id_str.clone(),
                None,
                resume_sid,
                cc_model_str.clone(),
                cc_effort_str.clone(),
                None,
            )
            .await;
        // Stale resume: CC process was expired and returned empty Result.
        // SessionEnded was emitted to prevent re-resume. Retry with a fresh
        // session — explicitly no resume_session_id, so the resolver starts
        // CC on a clean conversation rather than the sid we just proved
        // unusable.
        //
        // The fresh session gets a recap of the prior conversation, then a
        // *handoff brief* telling it how to rebuild the rest, then the message.
        let mut result = result;
        if let Err(ref e) = result {
            if e.to_string() == crate::engine::claude_code::STALE_RESUME_ERROR {
                log!("[Chat] Stale CC resume — retrying with fresh session");
                let brief = crate::engine::successor::handoff_brief(
                    thread_id,
                    crate::engine::successor::BriefCause::ResumeFailed,
                    Some(user_message),
                    None,
                );
                let retry_text = crate::engine::agent_session::prepend_reconstruction(
                    &self.pool, thread_id, &brief,
                )
                .await;
                // Same as above: still inside the unified router. This
                // is the stale-resume retry branch — fresh session with
                // a reconstructed conversation prepended. It runs on the same
                // claim, so nothing takes the thread between the two runs.
                result = self
                    .run_direct_agent(
                        &mut claim,
                        request_id,
                        &retry_text,
                        user_images,
                        origin_id,
                        &[],
                        spawning_event_id,
                        cancel_token,
                        conflict_claim.as_ref(),
                        None,
                        repo_id_str,
                        None,
                        None,
                        cc_model_str,
                        cc_effort_str,
                        None,
                    )
                    .await;
            }
        }
        result
    }
}
