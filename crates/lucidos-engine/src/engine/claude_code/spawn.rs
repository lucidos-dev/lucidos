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
                    &[],
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
            // running /harden, the change row is already applied and the
            // marker has been consumed by the apply. Both arms of the match
            // below would otherwise emit a spurious `ChangeApplyFailed` ~15s
            // after the user saw `ChangeApplied` (Ok arm via the false
            // `branch_is_hardened` post-check; Err arm via the
            // "Hardening failed" fallthrough).
            let concurrent_status = match engine.changes().get_by_id(change_id).await {
                Ok(change) => change.map(|c| c.status()),
                Err(e) => {
                    log!(
                        "[ClaudeCode] Concurrent-apply check: get_by_id({}): {}. Treating as not applied",
                        change_id,
                        e
                    );
                    None
                }
            };
            if change_applied_concurrently(concurrent_status) {
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
                        // Log only: `apply_change` announces its own failures,
                        // and a second emit draws the same card twice.
                        Err(e) => {
                            log!(
                                "[ClaudeCode] Auto-apply failed after hardening for change {}: {}",
                                change_id,
                                e
                            );
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
        // Copy out and RELEASE, before anything awaits. `agent_sessions` is
        // engine-global: every run-loop iteration, Stop, Apply and permission
        // answer takes it. The fallback below is a full teardown. It runs
        // several git calls, each bounded only by the 30s ceiling. Holding the
        // guard across it freezes every coding-agent session in the workspace
        // on one Discard click. `control.rs` drops the guard before the same
        // call.
        let target = {
            let mut guard = self.agent_sessions.lock().await;
            super::claim_for_discard(&mut guard, thread_id)?
        };

        let super::DiscardTarget::Claimed { worktree, claimant } = target else {
            // No live session, so fall back to stale session handling.
            // discard=true because this is the user-clicked Discard
            // button: explicit user intent.
            return self.end_stale_waiting_session(thread_id, true, actor).await;
        };

        let claim = crate::engine::agent_session::ChangeClaimGuard::new(
            self.agent_sessions.clone(),
            thread_id,
            claimant,
        );

        self.discard_pending_for_thread(thread_id, actor).await;

        self.reset_worktree_and_idle(thread_id, &worktree).await;

        claim.release().await;

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
            model,
            reasoning_effort,
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
        // Naming is decided here and carried out by `process_message_with_steps`
        // below, which writes the caller's name after the thread's row exists.
        // A title this spawn emitted itself would land before the row and be
        // dropped.
        let naming = crate::engine::chat::spawn_naming(caller_title.as_deref(), &prompt);

        let engine = self;
        let prompt_owned = prompt;
        let images_owned = user_images;
        let device_id_owned = device_id;
        let repo_id_owned = repo_id;

        {
            // Transient: parent's UI immediately renders the new CC sub-thread.
            // It carries the placeholder because the thread has no row yet, so
            // nothing durable can hold a name for it either.
            engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id: cc_thread_id,
                        event:
                            crate::engine::thread_events::ThreadEvent::CodingAgentThreadSpawned {
                                cc_thread_id: cc_thread_id.to_string(),
                                title: naming.placeholder,
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
                    // `reasoning_effort`: the caller's pin, or None to inherit
                    // the backend default. `resolve_route_overrides` passes a
                    // coding-agent effort straight through: the tier belongs to
                    // CC / Codex, not to the chat registry.
                    reasoning_effort.as_deref(),
                    // `provider_override`: always None. A coding-agent thread
                    // runs on its own backend, not on a chat-registry route.
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
                    // `cc_model` — the caller's pin, validated against this
                    // backend's picker at the tool boundary. It reaches
                    // `run_direct_agent`'s explicit-param slot, which wins over
                    // the session and thread-event fallbacks, so a spawn runs on
                    // the model the caller named rather than on CC's own
                    // default.
                    model.as_deref(),
                    Some(coding_agent),
                    None, // pre_emitted_origin — router emits MR itself
                    // The caller's chosen name. `maybe_emit_titles` writes it
                    // after `MessageReceived` creates the row, and skips the
                    // title model. `None` here reads as "nobody named this".
                    naming.caller_title.as_deref(),
                    origin,
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                    None,
                )
                .await;

            match result {
                Ok(ref res) => {
                    if res.proposed_change {
                        if res.auto_apply {
                            engine
                                .auto_apply_proposed_change(res.request_id, None)
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
    /// `ChangeApplyFailed` event on the thread, emitted by the apply path
    /// itself. Shared by the coding-agent and agent-chat Thread Queue
    /// execution paths.
    pub(crate) async fn auto_apply_proposed_change(
        self: &Arc<Self>,
        request_id: Uuid,
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
        match self.apply_change(change.id, actor).await {
            Ok(r) => {
                log!("[ClaudeCode] Auto-applied change: {}", r.message)
            }
            // Log only: `apply_change` announces its own failures.
            Err(e) => {
                log!("[ClaudeCode] Failed to auto-apply: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// This file with its test module cut off, through the shared reader the
    /// engine's other source scans use.
    fn production_src() -> String {
        crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::src_root().join("engine/claude_code/spawn.rs"),
        )
    }

    /// No teardown here awaits while holding the `agent_sessions` guard.
    ///
    /// `agent_sessions` is engine-global, so one held guard blocks every live
    /// run loop, the Stop button, Apply and every permission answer. A
    /// `return <expr>` evaluates its expression BEFORE the enclosing block's
    /// guard drops, which is how `discard_cc_changes` came to await a
    /// multi-minute teardown under the lock.
    ///
    /// A scan, because the failure is a wall-clock stall rather than a wrong
    /// value: nothing a unit test can assert on, and the shape is exact.
    #[test]
    fn no_stale_session_teardown_here_awaits_under_the_sessions_guard() {
        let src = production_src();
        for (at, _) in src.match_indices("end_stale_waiting_session(") {
            let Some(lock_at) = src[..at].rfind("self.agent_sessions.lock().await") else {
                continue;
            };
            if src[lock_at..at].contains("drop(guard)") {
                continue;
            }
            let closed = end_of_guard_block(&src, lock_at)
                .expect("the guard's enclosing block is brace-balanced");
            assert!(
                closed < at,
                "an `end_stale_waiting_session` await in spawn.rs is still inside \
                 the `agent_sessions` guard's block. Copy what you need out of the \
                 map, close the block, then await. See `control.rs`, which drops \
                 the guard before the same call."
            );
        }
    }

    /// Byte index just past the `}` closing the block that declares the guard.
    ///
    /// Braces are counted, never matched by substring. A closure in that span
    /// satisfies a scan for `};`, so such a scan passes the shape it exists to
    /// catch.
    ///
    /// It does not parse. A brace inside a string or a comment in that span
    /// would fool it, and the span carries neither.
    fn end_of_guard_block(src: &str, lock_at: usize) -> Option<usize> {
        let open = src[..lock_at].rfind('{')?;
        let mut depth = 0usize;
        for (offset, ch) in src[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open + offset + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }
}
