//! Direct Claude Code mode — the branch that runs CC against the current
//! thread (no spawn). Caller MUST keep the `ThreadGuard` from
//! `register_thread_queued` alive across the await — without it,
//! `cancel_thread` lands on `settle_stuck_running_thread` instead of the
//! per-thread `cancel_token` during the CC startup window (before
//! `agent_sessions` is populated), and CC keeps running.

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::types::ProcessResult;
use crate::engine::LucidosEngine;

impl LucidosEngine {
    /// Spawn / resume a CC turn for the current thread, coalescing rapid-fire
    /// follow-ups via the leader-election helper. Returns whatever
    /// `run_direct_agent` returns (or the queued-follower no-op `ProcessResult`
    /// if a leader is already in flight). Caller's `_guard` MUST stay alive
    /// across the await — `cancel_thread` lands on the per-thread token during
    /// the startup window before `agent_sessions` is populated.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_cc_chat_branch(
        &self,
        thread_id: Uuid,
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
        // True when the caller knows no rapid-fire follow-up can arrive
        // for this spawn (e.g. agent-spawned new thread via `run_thread`
        // tool). Skips the 250ms debounce + leader-election entirely.
        skip_coalesce: bool,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        log!(
            "[Chat] [TIMING] pre-CC overhead: {:?}",
            chat_start.elapsed()
        );
        let repo_id_str = repo_id.map(|s| s.to_string());
        let cc_model_str = cc_model.map(|s| s.to_string());
        let cc_effort_str = reasoning_effort.map(|s| s.to_string());

        // Coalesce rapid-fire follow-ups arriving while no Claude Code subprocess
        // is alive (Phase 2 made every CC exit on idle, so two messages
        // typed within ~250ms of each other both reach this branch).
        // The leader proceeds to spawn CC; followers queue their messages
        // for the leader to drain into a single combined input. Phase 5's
        // event-driven dispatcher will subsume this. See
        // `agent_session/cc_spawn_coalesce.rs` for the helper module.
        //
        // Skip coalescing for: (a) conflict-resolution turns — distinct
        // lifecycle (per-merge worktree), never user-typed in rapid
        // succession; (b) caller-flagged `skip_coalesce` (agent-spawned
        // new threads via `run_thread` — no user keyboard in the loop).
        const CC_SPAWN_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
        let coalesce = conflict_change_id.is_none() && !skip_coalesce;
        if coalesce {
            use crate::engine::agent_session::{LeaderElection, QueuedMessage};
            let my_msg = QueuedMessage {
                text: user_message.to_string(),
                images: user_images.map(|imgs| imgs.to_vec()),
                origin_event_id: Some(origin_id),
            };
            match self.cc_spawn_coalesce.try_become_leader(thread_id, my_msg) {
                LeaderElection::Queued => {
                    // Leader will pick up our message via drain. Our
                    // MessageReceived event is already persisted; the
                    // response will arrive via SSE for both turns.
                    log!(
                        "[Chat] CC follow-up for thread {} queued behind active spawn leader",
                        thread_id
                    );
                    return Ok(ProcessResult {
                        response: String::new(),
                        steps: vec![],
                        images: vec![],
                        request_id,
                        thread_id,
                        proposed_change: false,
                        auto_apply: false,
                        orphaned_injections: vec![],
                    });
                }
                LeaderElection::Leader => {
                    // Fall through to spawn CC. Drain happens just before
                    // run_direct_agent so we capture any followups that
                    // arrive during the brief debounce window.
                }
            }
        }

        // Brief debounce window: wait so any follow-ups typed within
        // CC_SPAWN_DEBOUNCE land in our queue before we spawn. Single
        // messages still pay this cost, but a CC spawn takes 5-10s of
        // process startup anyway, so the overhead is in the noise. Skip
        // when `coalesce` is false (conflict resolution / agent-spawn).
        if coalesce {
            tokio::time::sleep(CC_SPAWN_DEBOUNCE).await;
        }

        // Drain any queued followups and combine with our own message.
        let queued = if coalesce {
            self.cc_spawn_coalesce.drain_queue(thread_id)
        } else {
            Vec::new()
        };
        let coalesced_count = queued.len();
        let (combined_text, combined_images) = crate::engine::agent_session::combine_messages(
            user_message,
            user_images.map(|imgs| imgs.to_vec()),
            queued,
        );
        if coalesced_count > 0 {
            log!(
                "[Chat] Coalesced {} follow-up(s) into leader CC spawn for thread {}",
                coalesced_count,
                thread_id
            );
        }
        let combined_images_slice = combined_images.as_deref();

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
        // `process_message_with_steps_internal` (chat/process.rs's
        // `use_claude_code == Some(true)` branch routes here). The
        // unified router funnels every chat-driven and agent-driven CC
        // turn through this call; it's the bottom of the funnel.
        let result = self
            .run_direct_agent(
                request_id,
                thread_id,
                &combined_text,
                combined_images_slice,
                origin_id,
                spawning_event_id,
                cancel_token,
                conflict_change_id,
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
        // Phase 9: prepend a reconstruction of the prior conversation so
        // CC has rough context instead of starting from amnesia.
        let mut result = result;
        if let Err(ref e) = result {
            if e.to_string() == crate::engine::claude_code::STALE_RESUME_ERROR {
                log!("[Chat] Stale CC resume — retrying with fresh session");
                self.clear_cc_debounce(thread_id);
                let retry_text = crate::engine::agent_session::prepend_reconstruction(
                    &self.pool,
                    thread_id,
                    &combined_text,
                )
                .await;
                // Same as above: still inside the unified router. This
                // is the stale-resume retry branch — fresh session with
                // a reconstructed conversation prepended.
                result = self
                    .run_direct_agent(
                        request_id,
                        thread_id,
                        &retry_text,
                        combined_images_slice,
                        origin_id,
                        spawning_event_id,
                        cancel_token,
                        conflict_change_id,
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
        // Surface any follow-ups that arrived between drain and clear (a
        // small race window) as orphans so the caller re-routes them
        // rather than dropping them silently. Only relevant when we
        // participated in the coalesce map at all.
        if coalesce {
            let residual = self.cc_spawn_coalesce.clear(thread_id);
            if !residual.is_empty() {
                let mut orphans = crate::engine::agent_session::queued_to_orphans(residual);
                if let Ok(ref mut pr) = result {
                    pr.orphaned_injections.append(&mut orphans);
                }
            }
        }
        result
    }
}
