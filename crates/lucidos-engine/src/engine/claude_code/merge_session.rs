use super::*;
use crate::engine::agent_session::probe_merge_conflicts;
use crate::engine::change_ops::{CodingAgentChangeOps, LiveSessionInfo};

impl CodingAgentChangeOps for LucidosEngine {
    fn spawn_hardening(
        &self,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) {
        self.spawn_hardening_session(
            thread_id,
            worktree_path,
            branch_name,
            auto_apply_change_id,
            actor,
        );
    }

    async fn live_session_info(&self, thread_id: Uuid) -> Option<LiveSessionInfo> {
        let guard = self.agent_sessions.lock().await;
        guard.get(&thread_id).and_then(|s| {
            if s.process_exited {
                return None;
            }
            Some(LiveSessionInfo {
                worktree_path: s.worktree_path.clone()?,
                idle_notify: s.idle_notify.clone(),
                msg_tx: s.msg_tx.clone(),
                last_event_at: s.last_event_at.clone(),
            })
        })
    }

    fn spawn_merge_session(&self, thread_id: Uuid, change_id: Uuid, description: &str) {
        let engine = self.clone_arc();
        let description = description.to_string();
        Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
            let change = match engine.changes().get_by_id(change_id).await {
                Ok(Some(c)) => c,
                Ok(None) => {
                    log!(
                        "[MergeConflict] Change {} not found for spawn_merge_session",
                        change_id
                    );
                    return;
                }
                Err(e) => {
                    log!(
                        "[MergeConflict] get_by_id({}) failed: {} — skipping merge spawn",
                        change_id,
                        e
                    );
                    return;
                }
            };

            // `files` is the change's full file list, not necessarily
            // conflicting — Tier 2 has no worktree yet to probe.
            let prompt = engine
                .start_merge_and_get_prompt(
                    thread_id,
                    change_id,
                    change.files.clone(),
                    &change.branch_name,
                    Some("You are running in a temporary merge worktree."),
                    Some(&description),
                )
                .await;

            let origin_id = match engine.emit_automated_prompt(thread_id, &prompt, None).await {
                Ok(id) => id,
                Err(e) => {
                    log!(
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
            // Direct run_direct_agent (bypasses process_message_with_steps):
            // engine-driven Tier-1 merge orchestration. The prompt is
            // engine-synthesized via `start_merge_and_get_prompt`, and the
            // call needs `Some(change_id)` to bind CC to the change row.
            // process_message_with_steps has a conflict_change_id param but
            // also does its own MR emit + title gen + register_thread_queued,
            // which Tier-1 merge has already arranged via `emit_automated_prompt`.
            let result = engine
                .run_direct_agent(
                    request_id,
                    thread_id,
                    &prompt,
                    None,
                    origin_id,
                    None,
                    &cancel_token,
                    Some(change_id),
                    None,
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
                    log!(
                        "[MergeConflict] Claude Code session completed for change {}",
                        change_id
                    );
                    engine.broadcast_changes_updated().await;
                }
                Err(e) => {
                    log!(
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

    async fn lookup_session_id_for_resume(&self, thread_id: Uuid) -> Option<String> {
        // Shared lookup reads the id from both `CodingAgentIdled` and the
        // `Init`-time `CodingAgentSettingsChanged`, so a merge/apply resume of a
        // session interrupted before its first idle still `--resume`s instead of
        // falling back to a fresh session (DB errors log + yield None there).
        crate::engine::agent_session::lookup_latest_cc_session_id(self.pool(), thread_id).await
    }

    async fn run_merge_session_tier2(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        wt_path: &Path,
        branch_name: &str,
        description: &str,
        resume_token: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Probe the worktree for the actual conflicting files so the panel
        // shows real numbers — mirrors apply_now::merge_via_cc_session.
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
            .emit_automated_prompt(thread_id, &merge_prompt, None)
            .await?;
        let request_id = Uuid::new_v4();
        let cancel_token = tokio_util::sync::CancellationToken::new();

        // Direct run_direct_agent (bypasses process_message_with_steps):
        // engine-driven Tier-2 merge retry. Needs `recovery_worktree:
        // Some((wt_path, branch_name))` to run CC in the merge worktree
        // (not the main repo root) and `resume_token` to continue a prior
        // CC conversation if one exists. Neither concept exists in the
        // unified router's surface — chat doesn't have worktree-bound
        // sessions or CC `--resume` tokens.
        self.run_direct_agent(
            request_id,
            thread_id,
            &merge_prompt,
            None,
            origin_id,
            None,
            &cancel_token,
            Some(change_id),
            Some((wt_path.to_path_buf(), branch_name.to_string())),
            None,
            None,
            resume_token,
            None,
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }
}
