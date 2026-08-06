use super::*;
use crate::engine::thread_events::{ActorMode, EngineReason};

impl LucidosEngine {
    /// Spawn a Claude Code subprocess to run `/harden` on the given branch's worktree.
    /// When `auto_apply_change_id` is `Some`, the apply is re-entered after
    /// hardening completes (or `ChangeApplyFailed` is emitted on failure).
    /// `actor` carries the user who initiated the original apply so the
    /// resulting `ChangeApplied` stamps that user instead of the engine
    /// fallback.
    pub(crate) fn spawn_hardening_session(
        &self,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) {
        let engine = self.clone_arc();
        Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
            let prompt = "Your changes have not been hardened. Run /harden now.";

            // Engine-retriggered harden: stamp with the dedicated reason so the
            // route popover surfaces "Engine · Harden auto-retrigger" instead of
            // "Unknown".
            let origin = Some(MessageOrigin::engine(EngineReason::HardenRetrigger));
            let origin_id = match engine
                .emit_automated_prompt(thread_id, prompt, origin)
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    log!(
                        "[ClaudeCode] Failed to emit hardening prompt for thread {}: {}",
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
                            "[ClaudeCode] ResponseFailed",
                        )
                        .await;
                    return;
                }
            };

            let request_id = Uuid::new_v4();
            let cancel_token = tokio_util::sync::CancellationToken::new();

            let hardening_system_prompt = format!(
                "HARDENING SESSION: You are hardening changes on branch `{}`. \
                 Your ONLY task is to run the /harden skill. Do NOT continue any implementation work, \
                 do NOT look at implementation plans, do NOT add features. \
                 Just harden the existing changes for quality and correctness.\n\n\
                 CRITICAL: Never run `exit` as a bash command.",
                branch_name
            );
            // Direct run_direct_agent (bypasses process_message_with_steps):
            // engine-driven auto-harden. Needs `system_prompt_override` to
            // swap CC's normal system prompt for the hardening-only directive
            // above, and `recovery_worktree: Some((worktree_path, branch_name))`
            // to reuse the same change worktree. Neither hook exists on the
            // unified router's surface; this is engine plumbing, not a user
            // turn.
            let result = engine
                .run_direct_agent(
                    request_id,
                    thread_id,
                    prompt,
                    None,
                    origin_id,
                    None,
                    &cancel_token,
                    None,
                    Some((worktree_path, branch_name)), // recovery_worktree — reuse existing worktree
                    None,
                    Some(hardening_system_prompt),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;

            let Some(change_id) = auto_apply_change_id else {
                match result {
                    Ok(_) => log!(
                        "[ClaudeCode] Hardening session completed for thread {}",
                        thread_id
                    ),
                    Err(e) => log!(
                        "[ClaudeCode] Hardening session failed for thread {}: {}",
                        thread_id,
                        e
                    ),
                }
                return;
            };

            // Idempotency: if another device clicked Apply while CC was
            // running /harden, the change row is already `"applied"` and the
            // marker has been consumed by the apply. Both arms of the match
            // below would otherwise emit a spurious `ChangeApplyFailed` ~15s
            // after the user saw `ChangeApplied` (Ok arm via the false
            // `branch_is_hardened` post-check; Err arm via the
            // "Hardening failed" fallthrough).
            let concurrent_status = engine
                .changes()
                .get_by_id(change_id)
                .await
                .ok()
                .flatten()
                .map(|c| c.status);
            if change_applied_concurrently(concurrent_status.as_deref()) {
                log!(
                    "[ClaudeCode] Hardening session for change {} found change already applied (concurrent apply) — skipping post-CC failure emit",
                    change_id
                );
                engine.broadcast_changes_updated().await;
                return;
            }

            match result {
                Ok(_) => {
                    let was_aborted = cancel_token.is_cancelled();
                    let hardened = match engine.changes().get_by_id(change_id).await {
                        Ok(Some(c)) => {
                            crate::engine::change_ops::branch_is_hardened(
                                &engine.pool,
                                engine.changes(),
                                std::path::Path::new(&c.repo_root),
                                &c.branch_name,
                            )
                            .await
                        }
                        Ok(None) => false,
                        Err(e) => {
                            log!(
                                "[ClaudeCode] Hardening check: get_by_id({}): {} — treating as unhardened",
                                change_id,
                                e
                            );
                            false
                        }
                    };

                    if !hardening_succeeded(was_aborted, hardened) {
                        log!(
                            "[ClaudeCode] Hardening for change {} did not succeed (aborted={}, hardened={}) — skipping apply",
                            change_id,
                            was_aborted,
                            hardened
                        );
                        engine
                            .emit_apply_failed_unhardened(
                                thread_id,
                                &change_id.to_string(),
                                actor.clone(),
                                "[ClaudeCode] ChangeApplyFailed (incomplete hardening)",
                            )
                            .await;
                        engine.broadcast_changes_updated().await;
                        return;
                    }

                    log!(
                        "[ClaudeCode] Hardening completed for change {}, auto-applying",
                        change_id
                    );
                    // Emit ChangeHardened so apply_change doesn't re-enter the
                    // unhardened path and respawn hardening.
                    engine
                        .emit_change_hardened(thread_id, change_id, "[ClaudeCode] ChangeHardened")
                        .await;
                    match engine.apply_change(change_id, actor.clone()).await {
                        Ok(r) => {
                            log!(
                                "[ClaudeCode] Auto-applied change {} after hardening: {}",
                                change_id,
                                r.message
                            );
                            engine.broadcast_changes_updated().await;
                        }
                        Err(e) => {
                            log!(
                                "[ClaudeCode] Auto-apply failed after hardening for change {}: {}",
                                change_id,
                                e
                            );
                            engine
                                .event_bus
                                .emit_or_log(
                                    crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event:
                                            crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                                change_id: change_id.to_string(),
                                                error: e.to_string(),
                                                actor: actor.clone(),
                                            },
                                        meta: crate::engine::thread_events::EventMeta::NONE,
                                    },
                                    "[ClaudeCode] ChangeApplyFailed",
                                )
                                .await;
                        }
                    }
                }
                Err(e) => {
                    log!(
                        "[ClaudeCode] Hardening session failed for change {}: {}",
                        change_id,
                        e
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                        change_id: change_id.to_string(),
                                        error: format!("Hardening failed: {}", e),
                                        actor: actor.clone(),
                                    },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[ClaudeCode] ChangeApplyFailed (hardening)",
                        )
                        .await;
                }
            }
        });
    }

    /// Discard pending CC changes without ending the session.
    /// Resets the worktree to main and re-enters idle state.
    ///
    /// `actor` is the user who clicked Discard — propagated to any
    /// `ChangeApplyFailed` emitted by the stale-session fallback so the
    /// resulting event carries the real actor.
    pub async fn discard_cc_changes(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let wt = {
            let guard = self.agent_sessions.lock().await;
            if let Some(session) = guard.get(&thread_id) {
                session
                    .worktree_path
                    .clone()
                    .ok_or("No worktree for this session")?
            } else {
                // No live session — fall back to stale session handling.
                // discard=true because this is the user-clicked Discard
                // button: explicit user intent.
                return self.end_stale_waiting_session(thread_id, true, actor).await;
            }
        };

        self.discard_pending_for_thread(thread_id, actor).await;

        self.reset_worktree_and_idle(thread_id, &wt).await;

        self.broadcast_changes_updated().await;

        Ok(())
    }

    /// Run a coding-agent sub-thread spawn (the `run_coding_agent` LLM tool's work,
    /// executed by the Thread Queue once the entry is admitted). Routes the
    /// actual coding-agent work through `process_message_with_steps` (the unified
    /// router), mirroring the chat parallel `spawn_thread`. The future
    /// resolves when the session's turn finishes — the queue executor awaits
    /// it (wrapped in `monitor_cc_task` for panic cleanup) so the spawn's
    /// capacity slot is held for the session's duration.
    ///
    /// `cc_thread_id` is pre-allocated by the submitter so the tool result
    /// can return it without waiting for admission.
    pub(crate) async fn run_agent_thread_spawn(
        self: Arc<Self>,
        params: SpawnAgentThreadParams,
        cc_thread_id: Uuid,
    ) {
        let SpawnAgentThreadParams {
            prompt,
            user_images,
            device_id,
            parent_thread_id,
            spawning_event_id,
            repo_id,
            caller_title,
            app_id,
            coding_agent,
            origin,
        } = params;

        // Stash the app id for run_direct_agent to pick up. Cleared by the
        // run_direct_agent path once the spawn target is resolved.
        if let Some(ref a) = app_id {
            let mut guard = self
                .pending_app_spawn
                .lock()
                .expect("pending_app_spawn poisoned");
            guard.insert(cc_thread_id, a.clone());
        }
        let explicit_title = caller_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        let has_explicit_title = explicit_title.is_some();
        let initial_title =
            explicit_title.unwrap_or_else(|| prompt.chars().take(60).collect::<String>());

        let engine = self;
        let prompt_owned = prompt;
        let images_owned = user_images;
        let device_id_owned = device_id;
        let repo_id_owned = repo_id;

        {
            // Emit placeholder title + spawn LLM title gen. Matches
            // `spawn_thread` (chat parallel) — the placeholder shows up
            // before the LLM-generated title arrives, and
            // `process_message_with_steps`' follow-up title gen sees
            // `thread_has_title=true` and skips its own LLM call.
            if let Err(e) = engine
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id: cc_thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ThreadTitleGenerated {
                        title: initial_title.clone(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                })
                .await
            {
                log!("[ClaudeCode] Failed to emit title: {}", e);
            }

            // Generate the LLM title unless the caller supplied one. Built
            // here in the async task (rather than synchronously before the
            // spawn) so it can honor the `model_title` preference, matching
            // the chat path; an unset/empty preference falls back to the
            // extractor's default model.
            if !has_explicit_title {
                if let Some(ref extractor) = engine.extractor {
                    let title_model = crate::core::PreferenceStore::get(
                        &engine.pool,
                        crate::core::PREF_MODEL_TITLE,
                    )
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                    match extractor.provider_for_model(&title_model) {
                        Ok(provider) => {
                            let bus = engine.event_bus.clone();
                            let msg = prompt_owned.clone();
                            tokio::spawn(async move {
                                // Title is text-only: CC threads never generate
                                // image descriptions (no ImageDescribed event),
                                // so there's none to fold in even when the
                                // prompt carried images.
                                crate::engine::chat::emit_generated_title(
                                    &bus,
                                    provider.as_ref(),
                                    cc_thread_id,
                                    &msg,
                                    None,
                                    None,
                                    0,
                                )
                                .await;
                            });
                        }
                        Err(e) => {
                            log!("[ClaudeCode] Failed to build title provider: {}", e)
                        }
                    }
                }
            }

            // Transient: parent's UI immediately renders the new CC sub-thread.
            engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id: cc_thread_id,
                        event:
                            crate::engine::thread_events::ThreadEvent::CodingAgentThreadSpawned {
                                cc_thread_id: cc_thread_id.to_string(),
                                title: initial_title,
                                coding_agent,
                            },
                        meta: crate::engine::thread_events::EventMeta::NONE,
                    },
                    "[ClaudeCode] CodingAgentThreadSpawned",
                )
                .await;

            // Route the actual CC work through the unified router. Mirrors
            // `spawn_thread` (the chat parallel for sub-thread fan-out): the
            // slow path emits MessageReceived (with channel=ClaudeCode and the
            // `ThreadLink { mode: Agent }` origin the spawn site stamped, which
            // names the launching thread whatever the relation), registers the
            // thread, runs `run_cc_chat_branch` with `skip_coalesce=true`
            // (computed from mode=Agent + parent linkage, so a top spawn keeps
            // coalescing), and dispatches to `run_direct_agent`.
            let result = engine
                .process_message_with_steps(
                    &prompt_owned,
                    None,
                    None,
                    None,
                    None,
                    images_owned.as_deref(),
                    device_id_owned.as_deref(),
                    Some(true),
                    None,
                    Some(cc_thread_id),
                    None,
                    repo_id_owned.as_deref(),
                    None,
                    parent_thread_id,
                    spawning_event_id,
                    ActorMode::Agent,
                    None,
                    Some(coding_agent),
                    None, // pre_emitted_origin — router emits MR itself
                    None, // title — already emitted placeholder above
                    origin,
                    crate::engine::FollowUpUrgency::Normal,
                )
                .await;

            match result {
                Ok(ref res) => {
                    if res.proposed_change {
                        if res.auto_apply {
                            engine
                                .auto_apply_proposed_change(res.request_id, cc_thread_id, None)
                                .await;
                        }

                        engine.broadcast_changes_updated().await;
                    }
                }
                Err(e) => {
                    log!("[ClaudeCode] Background Claude Code session failed: {}", e);
                    emit_background_task_failure(
                        &engine,
                        cc_thread_id,
                        &e,
                        "[ClaudeCode] run_agent_thread_spawn failure",
                    )
                    .await;
                }
            }
        }
    }

    /// Look up the pending change a finished background turn proposed (by
    /// `request_id`) and apply it; an apply failure surfaces as a
    /// `ChangeApplyFailed` event on the thread. Shared by the coding-agent
    /// and agent-chat Thread Queue execution paths.
    pub(crate) async fn auto_apply_proposed_change(
        self: &Arc<Self>,
        request_id: Uuid,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        let pending = match self.changes().list_pending().await {
            Ok(v) => v,
            Err(e) => {
                log!(
                    "[ClaudeCode] auto-apply: list_pending: {} — skipping auto-apply",
                    e
                );
                Vec::new()
            }
        };
        let Some(change) = pending.iter().find(|c| c.request_id == request_id) else {
            return;
        };
        match self.apply_change(change.id, actor.clone()).await {
            Ok(r) => {
                log!("[ClaudeCode] Auto-applied change: {}", r.message)
            }
            Err(e) => {
                log!("[ClaudeCode] Failed to auto-apply: {}", e);
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                change_id: change.id.to_string(),
                                error: e.to_string(),
                                actor,
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[ClaudeCode] ChangeApplyFailed",
                    )
                    .await;
            }
        }
    }
}
