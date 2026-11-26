use super::*;
use crate::engine::agent_recovery::recovery::AgentRecoveryExt;
use crate::engine::agent_session::apply_now::ApplyNowExt;
use crate::engine::agent_session::run_session::run::RunAgentSessionExt;
use crate::engine::agent_session::runtime_helpers::RuntimeHelpersExt;
use crate::engine::agent_session::wait_for_prompt_turn;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{ActorMode, EngineReason};
use std::time::Duration;

const HARDEN_TURN_SETTLE_LIMIT: Duration = Duration::from_secs(120);

const HARDEN_TURN_UNSETTLED: &str = "Hardening finished, but its thread did not settle in time, \
     so the change was not applied. Apply it again.";

const SETTLE_RECHECK: Duration = Duration::from_secs(5);

pub(crate) enum HardeningRun {
    Ended {
        aborted: bool,
    },
    Failed(String),
    /// INVARIANT: The run's thread did not settle in time, or its settle could not be
    /// read. Applying could merge a branch its proposal still writes.
    Unsettled,
}

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
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        std::future::pending::<()>().await
                    }
                }
            }
        };
        let _ = tokio::time::timeout(left.min(SETTLE_RECHECK), next_wake).await;
    }
}

#[easy_ext::ext(ClaudeCodeSpawnExt)]
pub(crate) impl LucidosEngine {
    fn spawn_hardening_session(
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
            let result = match engine.claim_thread_or_refuse(thread_id).await {
                Err(e) => Err(e),
                Ok(mut claim) => {
                    let origin_id = match engine
                        .emit_automated_prompt(thread_id, prompt, EngineReason::HardenRetrigger)
                        .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            log_legacy!(
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
                                            spend_cap: None,
                                            agent_exit: None,
                                            provider_error: None,
                                        },
                                        meta: crate::engine::thread_events::EventMeta::NONE,
                                    },
                                    "[ClaudeCode] ResponseFailed",
                                )
                                .await;
                            return;
                        }
                    };

                    // INVARIANT: Direct run_direct_agent (bypasses process_message_with_steps):
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
                            Some((worktree_path, branch_name)),
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
                    Ok(_) => log_legacy!(
                        "[ClaudeCode] Hardening session completed for thread {}",
                        thread_id
                    ),
                    Err(e) => log_legacy!(
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

    /// INVARIANT: Finish an Apply that routed `/harden` through the thread's live
    /// session, whose own turn end proposes but never applies. Re-enters the
    /// apply once that turn has settled. Its idle follows the proposal, and an
    /// earlier apply would race the proposal on the same branch.
    fn spawn_apply_after_session_harden(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        answered: tokio::sync::oneshot::Receiver<crate::engine::InputAnswered>,
        actor: Option<MessageOrigin>,
    ) {
        let engine = self.clone_arc();
        let mut wakes = self.event_bus.subscribe();
        tokio::spawn(async move {
            if let Err(e) = wait_for_prompt_turn(answered, "waiting for hardening").await {
                log_legacy!("[ClaudeCode] Hardening for change {}: {}", change_id, e);
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
                log_legacy!(
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

    async fn agent_turn_settled(&self, thread_id: Uuid) -> bool {
        use crate::engine::standing_apply::{read_turn_settle, TurnSettle};
        matches!(
            read_turn_settle(&self.pool, thread_id, &self.agent_sessions).await,
            Some(TurnSettle::Settled | TurnSettle::CutOff)
        )
    }

    async fn apply_after_hardening(
        self: &Arc<Self>,
        thread_id: Uuid,
        change_id: Uuid,
        run: HardeningRun,
        actor: Option<MessageOrigin>,
    ) {
        // INVARIANT: Idempotency: if another device clicked Apply while CC was
        // running /harden, the change row is already applied and the
        // marker has been consumed by the apply. Every arm below would
        // otherwise report a spurious failure after the user saw
        // `ChangeApplied`.
        let concurrent_status = match self.changes().get_by_id(change_id).await {
            Ok(change) => change.map(|c| c.status()),
            Err(e) => {
                log_legacy!(
                    "[ClaudeCode] Concurrent-apply check: get_by_id({}): {}. Treating as not applied",
                    change_id,
                    e
                );
                None
            }
        };
        if change_applied_concurrently(concurrent_status) {
            log_legacy!(
                "[ClaudeCode] Hardening for change {} found it already applied (concurrent apply), skipping the post-hardening apply",
                change_id
            );
            self.broadcast_changes_updated().await;
            return;
        }

        let was_aborted = match run {
            HardeningRun::Ended { aborted } => aborted,
            HardeningRun::Failed(e) => {
                log_legacy!(
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
                log_legacy!(
                    "[ClaudeCode] Hardening check: get_by_id({}): {}. Treating as unhardened",
                    change_id,
                    e
                );
                false
            }
        };

        if !hardening_succeeded(was_aborted, hardened) {
            log_legacy!(
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

        log_legacy!(
            "[ClaudeCode] Hardening completed for change {}, auto-applying",
            change_id
        );
        self.emit_change_hardened(thread_id, change_id, "[ClaudeCode] ChangeHardened")
            .await;
        match self.apply_change(change_id, actor).await {
            Ok(r) => {
                log_legacy!(
                    "[ClaudeCode] Auto-applied change {} after hardening: {}",
                    change_id,
                    r.message
                );
                self.broadcast_changes_updated().await;
            }
            Err(e) => {
                log_legacy!(
                    "[ClaudeCode] Auto-apply failed after hardening for change {}: {}",
                    change_id,
                    e
                );
            }
        }
    }

    async fn discard_cc_changes(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    /// INVARIANT: Run a coding-agent sub-thread spawn (the `run_coding_agent` LLM tool's work,
    /// executed by the Thread Queue once the entry is admitted). Routes the
    /// actual coding-agent work through `process_message_with_steps` (the unified
    /// router), mirroring the chat parallel `spawn_thread`. The future
    /// resolves when the session's turn finishes — the queue executor awaits
    /// it (wrapped in `monitor_cc_task` for panic cleanup) so the spawn's
    /// capacity slot is held for the session's duration.
    ///
    /// `cc_thread_id` is pre-allocated by the submitter so the tool result
    /// can return it without waiting for admission.
    async fn run_agent_thread_spawn(
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

        if let Some(ref a) = app_id {
            let mut guard = self
                .pending_app_spawn
                .lock()
                .expect("pending_app_spawn poisoned");
            guard.insert(cc_thread_id, a.clone());
        }
        // INVARIANT: Naming is decided here and carried out by `process_message_with_steps`
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

            let result = engine
                .process_message_with_steps(
                    &prompt_owned,
                    None,
                    None,
                    None,
                    reasoning_effort.as_deref(),
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
                    model.as_deref(),
                    Some(coding_agent),
                    None,
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
                    // INVARIANT: Drain the orphan chain, as every other spawn site does. A
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
                    log_legacy!("[ClaudeCode] Background Claude Code session failed: {}", e);
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

    async fn auto_apply_proposed_change(
        self: &Arc<Self>,
        request_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        let pending = match self.changes().list_pending().await {
            Ok(v) => v,
            Err(e) => {
                log_legacy!(
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
                log_legacy!("[ClaudeCode] Auto-applied change: {}", r.message)
            }
            Err(e) => {
                log_legacy!("[ClaudeCode] Failed to auto-apply: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

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
            tokio::time::sleep(Duration::from_secs(1)).await;
            flip.store(true, Ordering::SeqCst);
            tx.send(7).unwrap();
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

    #[tokio::test(start_paused = true)]
    async fn a_lagged_bus_re_asks_at_once_instead_of_waiting_out_the_tick() {
        let (tx, mut wakes) = tokio::sync::broadcast::channel::<u32>(2);
        let done = Arc::new(AtomicBool::new(false));
        let flip = done.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            flip.store(true, Ordering::SeqCst);
            for n in 0..5 {
                tx.send(n).unwrap();
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        let started = tokio::time::Instant::now();
        let settled = wait_until_settled(
            || {
                let done = done.clone();
                async move { done.load(Ordering::SeqCst) }
            },
            &mut wakes,
            |_| false,
            Duration::from_secs(600),
        )
        .await;
        assert!(settled);
        assert!(
            started.elapsed() < SETTLE_RECHECK,
            "skipped events may hold the settle, so the wait re-asks without the tick"
        );
    }

    mod guard_tests {
        use super::*;

        fn body_between(src: &str, from: &str, to: &str) -> String {
            let start = src.find(from).unwrap_or_else(|| panic!("no {from}"));
            let rest = &src[start..];
            rest[..rest.find(to).unwrap_or_else(|| panic!("no {to}"))].to_string()
        }

        #[test]
        fn hardening_claims_the_thread_before_it_emits_the_prompt() {
            let body = body_between(
                &production_src(),
                "fn spawn_hardening_session(",
                "fn spawn_apply_after_session_harden(",
            );
            let claim = body.find("claim_thread_or_refuse(").expect("the claim");
            let emit = body.find("emit_automated_prompt(").expect("the prompt");
            assert!(
                claim < emit,
                "a live run must refuse before the prompt lands"
            );
        }

        #[test]
        fn the_session_harden_wait_subscribes_before_its_task_starts() {
            let body = body_between(
                &production_src(),
                "fn spawn_apply_after_session_harden(",
                "async fn agent_turn_settled(",
            );
            let subscribe = body
                .find("event_bus.subscribe()")
                .expect("the subscription");
            let spawn = body.find("tokio::spawn(").expect("the task");
            assert!(subscribe < spawn, "the idle event could slip past the task");
        }

        fn production_src() -> String {
            crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("engine/claude_code/spawn.rs"),
            )
        }

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
    }
}
