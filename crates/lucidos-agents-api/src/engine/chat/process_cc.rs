//! INVARIANT: Direct Claude Code mode: the branch that runs a coding agent against the
//! current thread. The caller claimed the thread at its fast path, so a
//! follow-up sent during startup queues for this run and a Stop cancels it.
//! The caller's `ThreadGuard` stays alive across the await: the run's
//! `chat_cancel` arm listens on its token for a request abort or a shutdown.

use crate::engine::agent_session::run_session::run::RunAgentSessionExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::session_claim::SpawnClaim;
use crate::engine::types::ProcessResult;
use crate::engine::LucidosEngine;

#[easy_ext::ext(ProcessCcExt)]
pub(crate) impl LucidosEngine {
    #[allow(clippy::too_many_arguments)]
    async fn run_cc_chat_branch(
        &self,
        mut claim: SpawnClaim,
        request_id: Uuid,
        user_message: &str,
        user_images: Option<&[crate::engine::types::ChatImage]>,
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
        log_legacy!(
            "[Chat] [TIMING] pre-CC overhead: {:?}",
            chat_start.elapsed()
        );
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

        let resume_sid =
            crate::engine::session_queries::lookup_latest_cc_session_id(&self.pool, thread_id)
                .await;
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
        let mut result = result;
        if let Err(ref e) = result {
            if e.to_string() == crate::engine::claude_code::STALE_RESUME_ERROR {
                log_legacy!("[Chat] Stale CC resume — retrying with fresh session");
                let retry_text = crate::engine::successor::resume_failed_input(
                    &self.pool,
                    thread_id,
                    user_message,
                )
                .await;
                // INVARIANT: Same as above: still inside the unified router. This
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
