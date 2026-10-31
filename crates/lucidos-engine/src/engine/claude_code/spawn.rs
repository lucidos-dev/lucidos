use super::*;
use crate::engine::agent_session::wait_for_prompt_turn;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{ActorMode, EngineReason};
use std::time::Duration;

/// How long a hardening turn may take from its end to its idle before the
/// apply gives up. The run loop only proposes in between.
const HARDEN_TURN_SETTLE_LIMIT: Duration = Duration::from_secs(120);

const HARDEN_TURN_UNSETTLED: &str = "Hardening finished, but its thread did not settle in time, \
     so the change was not applied. Apply it again.";

/// How often a settle wait re-asks when no event on its thread arrives.
const SETTLE_RECHECK: Duration = Duration::from_secs(5);

/// How a hardening run ended, for [`LucidosEngine::apply_after_hardening`].
pub(crate) enum HardeningRun {
    /// The run finished. `aborted` is true when it was cancelled.
    Ended {
        aborted: bool,
    },
    Failed(String),
    /// The run's thread did not settle in time, or its settle could not be
    /// read. Applying could merge a branch its proposal still writes.
    Unsettled,
}

/// Wait until `settled` answers true, re-asking on each event `wakes` carries
/// that `is_wake` accepts and on every recheck tick. Returns false once
/// `limit` passes unsettled.
pub(crate) async fn wait_until_settled<T, F, Fut>(
    mut settled: F,
    wakes: &mut tokio::sync::broadcast::Receiver<T>,
    is_wake: impl Fn(&T) -> bool,
    limit: Duration,
) -> bool
where
    T: Clone,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if settled().await {
            return true;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        let next_wake = async {
            loop {
                match wakes.recv().await {
                    Ok(event) if is_wake(&event) => return,
                    Ok(_) => {}
                    // A skipped event may be the settle itself.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return,
                    // Only the tick wakes a closed bus.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        std::future::pending::<()>().await
                    }
                }
            }
        };
        let _ = tokio::time::timeout(left.min(SETTLE_RECHECK), next_wake).await;
    }
}

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
            // Claimed before the prompt is emitted, so a live run on the
            // thread refuses the hardening instead of sharing its worktree.
            let result = match engine.claim_thread_or_refuse(thread_id).await {
                Err(e) => Err(e),
                Ok(mut claim) => {
                    let origin_id = match engine
                        .emit_automated_prompt(thread_id, prompt, EngineReason::HardenRetrigger)
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
                                            agent_exit: None,
                                        },
                                        meta: crate::engine::thread_events::EventMeta::NONE,
                                    },
                                    "[ClaudeCode] ResponseFailed",
                                )
                                .await;
                            return;
                        }
                    };

                    // Direct run_direct_agent (bypasses process_message_with_steps):
                    // engine-driven auto-harden. Needs `system_prompt_override` to
                    // swap CC's normal system prompt for the hardening-only directive
                    // above, and `recovery_worktree: Some((worktree_path, branch_name))`
                    // to reuse the same change worktree. Neither hook exists on the
                    // unified router's surface; this is engine plumbing, not a user
                    // turn.
                    engine
                        .run_direct_agent(
                            &mut claim,
                            request_id,
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
                        )
                        .await
                }
            };

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

            let run = match result {
                Ok(_) => HardeningRun::Ended {
                    aborted: cancel_token.is_cancelled(),
                },
                Err(e) => HardeningRun::Failed(e.to_string()),
            };
            engine
                .apply_after_hardening(thread_id, change_id, run, actor)
                .await;
        });
    }

    /// Finish an Apply that routed `/harden` through the thread's live
    /// session, whose own turn end proposes but never applies. Re-enters the
    /// apply once that turn has settled. Its idle follows the proposal, and an
    /// earlier apply would race the proposal on the same branch.
    pub(crate) fn spawn_apply_after_session_harden(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        answered: tokio::sync::oneshot::Receiver<crate::engine::InputAnswered>,
        actor: Option<MessageOrigin>,
    ) {
        let engine = self.clone_arc();
        // Subscribed before the task starts, so the idle cannot slip past it.
        let mut wakes = self.event_bus.subscribe();
        tokio::spawn(async move {
            // A rejected prompt or an ended session ran no `/harden`, and the
            // marker check in `apply_after_hardening` reports that.
            if let Err(e) = wait_for_prompt_turn(answered, "waiting for hardening").await {
                log!("[ClaudeCode] Hardening for change {}: {}", change_id, e);
            }
            let settled = wait_until_settled(
                || engine.agent_turn_settled(thread_id),
                &mut wakes,
                |event| match &event.typed {
                    BusEvent::Thread { thread_id: t, .. } => *t == thread_id,
                    BusEvent::System(_) => false,
                },
                HARDEN_TURN_SETTLE_LIMIT,
            )
            .await;
            let run = if settled {
                HardeningRun::Ended { aborted: false }
            } else {
                log!(
                    "[ClaudeCode] Thread {} did not settle within {:?} of its hardening turn; \
                     not applying change {}",
                    thread_id,
                    HARDEN_TURN_SETTLE_LIMIT,
                    change_id
                );
                HardeningRun::Unsettled
            };
            engine
                .apply_after_hardening(thread_id, change_id, run, actor)
                .await;
        });
    }

    /// No agent turn on the thread sits between its terminal and its idle.
    /// An unanswered probe reads as unsettled, so the wait asks again.
    async fn agent_turn_settled(&self, thread_id: Uuid) -> bool {
        use crate::engine::standing_apply::{read_turn_settle, TurnSettle};
        matches!(
            read_turn_settle(&self.pool, thread_id, &self.agent_sessions).await,
            Some(TurnSettle::Settled | TurnSettle::CutOff)
        )
    }

    /// Re-enter an apply once its hardening run ended: apply the change when
    /// the marker landed, else report the apply failed. Every report reaches
    /// an Apply All batch the change belongs to.
    pub(crate) async fn apply_after_hardening(
        self: &Arc<Self>,
        thread_id: Uuid,
        change_id: Uuid,
        run: HardeningRun,
        actor: Option<MessageOrigin>,
    ) {
        // Idempotency: if another device clicked Apply while CC was
        // running /harden, the change row is already applied and the
        // marker has been consumed by the apply. Every arm below would
        // otherwise report a spurious failure after the user saw
        // `ChangeApplied`.
        let concurrent_status = match self.changes().get_by_id(change_id).await {
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
                "[ClaudeCode] Hardening for change {} found it already applied (concurrent apply), skipping the post-hardening apply",
                change_id
            );
            self.broadcast_changes_updated().await;
            return;
        }

        let was_aborted = match run {
            HardeningRun::Ended { aborted } => aborted,
            HardeningRun::Failed(e) => {
                log!(
                    "[ClaudeCode] Hardening session failed for change {}: {}",
                    change_id,
                    e
                );
                self.emit_apply_failed(
                    thread_id,
                    change_id,
                    &format!("Hardening failed: {}", e),
                    actor,
                )
                .await;
                return;
            }
            HardeningRun::Unsettled => {
                self.emit_apply_failed(thread_id, change_id, HARDEN_TURN_UNSETTLED, actor)
                    .await;
                return;
            }
        };
        let hardened = match self.changes().get_by_id(change_id).await {
            Ok(Some(c)) => {
                crate::engine::change_ops::branch_is_hardened(
                    &self.pool,
                    self.changes(),
                    std::path::Path::new(&c.repo_root),
                    &c.branch_name,
                )
                .await
            }
            Ok(None) => false,
            Err(e) => {
                log!(
                    "[ClaudeCode] Hardening check: get_by_id({}): {}. Treating as unhardened",
                    change_id,
                    e
                );
                false
            }
        };

        if !hardening_succeeded(was_aborted, hardened) {
            log!(
                "[ClaudeCode] Hardening for change {} did not succeed (aborted={}, hardened={}), skipping apply",
                change_id,
                was_aborted,
                hardened
            );
            self.emit_apply_failed_unhardened(
                thread_id,
                &change_id.to_string(),
                actor,
                "[ClaudeCode] ChangeApplyFailed (incomplete hardening)",
            )
            .await;
            self.broadcast_changes_updated().await;
            return;
        }

        log!(
            "[ClaudeCode] Hardening completed for change {}, auto-applying",
            change_id
        );
        // Emit ChangeHardened so apply_change doesn't re-enter the
        // unhardened path and respawn hardening.
        self.emit_change_hardened(thread_id, change_id, "[ClaudeCode] ChangeHardened")
            .await;
        match self.apply_change(change_id, actor).await {
            Ok(r) => {
                log!(
                    "[ClaudeCode] Auto-applied change {} after hardening: {}",
                    change_id,
                    r.message
                );
                self.broadcast_changes_updated().await;
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

    /// Discard pending CC changes without ending the session.
    /// Resets the worktree to main, if it is still on the session's branch,
    /// and re-enters idle state.
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

        let super::DiscardTarget::Claimed {
            worktree,
            branch,
            claimant,
        } = target
        else {
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

        self.discard_open_changes_for_thread(thread_id, actor).await;

        self.reset_worktree_and_idle(thread_id, &worktree, &branch)
            .await;

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
            wake_mode,
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
            // thread, and runs `run_cc_chat_branch`, which dispatches to
            // `run_direct_agent` on the thread's spawn claim.
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
                    wake_mode,
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
                Ok(res) => {
                    if res.proposed_change {
                        if res.auto_apply {
                            engine
                                .auto_apply_proposed_change(res.request_id, None)
                                .await;
                        }

                        engine.broadcast_changes_updated().await;
                    }
                    // Drain the orphan chain, as every other spawn site does. A
                    // follow-up that reached the session as it went idle comes
                    // back here, and dropping it leaves the message unanswered.
                    if !res.orphaned_injections.is_empty() {
                        crate::api::chat::process_orphan_chain(
                            engine.clone(),
                            res.thread_id,
                            res.orphaned_injections,
                        )
                        .await;
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
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

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

    #[tokio::test(start_paused = true)]
    async fn a_settle_wait_re_asks_on_its_threads_event() {
        let (tx, mut wakes) = tokio::sync::broadcast::channel::<u32>(8);
        let done = Arc::new(AtomicBool::new(false));
        let asked = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let flip = done.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            tx.send(9).unwrap();
            // Unsettled while the other thread's event is read.
            tokio::time::sleep(Duration::from_secs(1)).await;
            flip.store(true, Ordering::SeqCst);
            tx.send(7).unwrap();
            // Keeps the bus open, so only the event can end the wait early.
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        let started = tokio::time::Instant::now();
        let settled = wait_until_settled(
            || {
                let done = done.clone();
                let asked = asked.clone();
                async move {
                    asked.fetch_add(1, Ordering::SeqCst);
                    done.load(Ordering::SeqCst)
                }
            },
            &mut wakes,
            |event| *event == 7,
            Duration::from_secs(600),
        )
        .await;
        assert!(settled);
        assert!(
            started.elapsed() < SETTLE_RECHECK,
            "the event on the thread wakes the wait, not the recheck tick"
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            2,
            "another thread's event never re-asks"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_settle_wait_that_never_settles_gives_up_at_its_limit() {
        let (tx, mut wakes) = tokio::sync::broadcast::channel::<u32>(8);
        let limit = Duration::from_secs(60);
        let started = tokio::time::Instant::now();
        let settled = wait_until_settled(|| async { false }, &mut wakes, |_| true, limit).await;
        drop(tx);
        assert!(!settled);
        assert!(started.elapsed() >= limit);
        assert!(started.elapsed() < limit + SETTLE_RECHECK);
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_bus_still_re_asks_on_the_tick() {
        let (tx, mut wakes) = tokio::sync::broadcast::channel::<u32>(8);
        drop(tx);
        let asked = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count = asked.clone();
        let settled = wait_until_settled(
            || {
                let count = count.clone();
                async move { count.fetch_add(1, Ordering::SeqCst) >= 2 }
            },
            &mut wakes,
            |_| true,
            Duration::from_secs(60),
        )
        .await;
        assert!(settled);
        assert_eq!(asked.load(Ordering::SeqCst), 3, "one ask per tick, no spin");
    }
}
