use super::*;
use crate::engine::agent_session::probe_merge_conflicts;
use crate::engine::agent_session::run_session::run::RunAgentSessionExt;
use crate::engine::agent_session::runtime_helpers::RuntimeHelpersExt;
use crate::engine::thread_events::EngineReason;

#[easy_ext::ext(MergeSessionExt)]
pub(crate) impl LucidosEngine {
    fn spawn_merge_session(
        &self,
        thread_id: Uuid,
        apply_claim: crate::engine::change_ops::apply_claim::ApplyClaim,
        description: &str,
    ) {
        let engine = self.clone_arc();
        let description = description.to_string();
        let change_id = apply_claim.change_id();
        Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
            let mut spawn_claim = match engine.claim_thread_or_refuse(thread_id).await {
                Ok(claim) => claim,
                Err(e) => {
                    log_legacy!(
                        "[MergeConflict] No merge session for change {}: {}",
                        change_id,
                        e
                    );
                    let actor = engine.pending_apply_actors.take(change_id);
                    engine
                        .emit_apply_failed(thread_id, change_id, &e.to_string(), actor)
                        .await;
                    return;
                }
            };
            let change = match engine.changes().get_by_id(change_id).await {
                Ok(Some(c)) => c,
                Ok(None) => {
                    log_legacy!(
                        "[MergeConflict] Change {} not found for spawn_merge_session",
                        change_id
                    );
                    return;
                }
                Err(e) => {
                    log_legacy!(
                        "[MergeConflict] get_by_id({}) failed: {} — skipping merge spawn",
                        change_id,
                        e
                    );
                    return;
                }
            };

            let conflicts = crate::engine::git_ops::predict_merge_into_main(
                std::path::Path::new(&change.repo_root),
                &change.branch_name,
            )
            .await
            .into_conflicts();
            let prompt = engine
                .start_merge_and_get_prompt(
                    thread_id,
                    change_id,
                    conflicts,
                    &change.branch_name,
                    Some("You are running in a temporary merge worktree."),
                    Some(&description),
                )
                .await;

            let origin_id = match engine
                .emit_automated_prompt(thread_id, &prompt, EngineReason::MergeConflict)
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    log_legacy!(
                        "[MergeConflict] Failed to emit merge prompt for thread {}: {}",
                        thread_id,
                        e
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                    error: format!("Failed to emit prompt: {}", e),
                                    spend_cap: None,
                                    agent_exit: None,
                                    provider_error: None,
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[MergeConflict] ResponseFailed",
                        )
                        .await;
                    return;
                }
            };

            let request_id = Uuid::new_v4();
            let cancel_token = tokio_util::sync::CancellationToken::new();
            // INVARIANT: Direct run_direct_agent (bypasses process_message_with_steps):
            // engine-driven Tier-3 merge orchestration. The prompt is
            // engine-synthesized via `start_merge_and_get_prompt`, and the
            // session holds the change's apply claim for its whole run.
            // process_message_with_steps has a conflict_change_id param but
            // also does its own MR emit + title gen + register_thread_queued,
            // which this merge has already arranged via `emit_automated_prompt`.
            let result = engine
                .run_direct_agent(
                    &mut spawn_claim,
                    request_id,
                    &prompt,
                    None,
                    origin_id,
                    &[],
                    None,
                    &cancel_token,
                    Some(&apply_claim),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
            match result {
                Ok(_) => {
                    log_legacy!(
                        "[MergeConflict] Claude Code session completed for change {}",
                        change_id
                    );
                    engine.broadcast_changes_updated().await;
                }
                Err(e) => {
                    log_legacy!(
                        "[MergeConflict] Claude Code session failed for change {}: {}",
                        change_id,
                        e
                    );
                    emit_background_task_failure(
                        &engine,
                        thread_id,
                        &e,
                        "[MergeConflict] spawn_merge_session failure",
                    )
                    .await;
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_merge_session_tier2(
        &self,
        thread_id: Uuid,
        apply_claim: &crate::engine::change_ops::apply_claim::ApplyClaim,
        thread_hold: crate::engine::session_claim::SpawnClaim,
        wt_path: &Path,
        branch_name: &str,
        description: &str,
        resume_token: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let change_id = apply_claim.change_id();
        let mut spawn_claim = self.hand_apply_hold_to_run(thread_hold).await;
        let conflict_files = probe_merge_conflicts(wt_path).await;
        let merge_prompt = self
            .start_merge_and_get_prompt(
                thread_id,
                change_id,
                conflict_files,
                "main",
                None,
                Some(description),
            )
            .await;
        let origin_id = self
            .emit_automated_prompt(thread_id, &merge_prompt, EngineReason::MergeConflict)
            .await?;
        let request_id = Uuid::new_v4();
        let cancel_token = tokio_util::sync::CancellationToken::new();

        self.run_direct_agent(
            &mut spawn_claim,
            request_id,
            &merge_prompt,
            None,
            origin_id,
            &[],
            None,
            &cancel_token,
            Some(apply_claim),
            Some((wt_path.to_path_buf(), branch_name.to_string())),
            None,
            None,
            resume_token,
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }
}
