use super::super::*;

impl LucidosEngine {
    pub const DEFAULT_REPO_NAME: &'static str = "Lucidos";

    pub fn worktree_cleanup_active_threads(&self) -> Arc<dyn worktree_cleanup::ActiveThreads> {
        Arc::new(worktree_cleanup::EngineActiveThreads::new(
            self.agent_sessions.clone(),
            self.bash_background.clone(),
            self.terminals.clone(),
        ))
    }

    pub fn start_parent_callback_listener(self: &Arc<Self>) {
        let rx = PARENT_CALLBACK_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            crate::log_legacy!("[FanOut] No parent callback receiver found — listener not started");
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(cb) = rx.recv().await {
                let engine = engine.clone();
                tokio::spawn(async move {
                    crate::log_legacy!(
                        "[FanOut] Child {} completed, notifying parent {}",
                        cb.child_thread_id,
                        cb.parent_thread_id
                    );
                    if let Err(e) = engine
                        .notify_parent_of_child_completion(
                            cb.parent_thread_id,
                            cb.child_thread_id,
                            cb.child_completed_event_id,
                            cb.child_terminal_event_id,
                            cb.parent_is_coding_agent,
                        )
                        .await
                    {
                        crate::log_legacy!(
                            "[FanOut] Failed to notify parent {} of child {}: {}",
                            cb.parent_thread_id,
                            cb.child_thread_id,
                            e
                        );
                        // INVARIANT: The card already marked the parent awake, so a
                        // failure here would leave it reading "Requesting"
                        // with no turn behind it until the next boot. Report
                        // the failure instead: `ResponseFailed` settles the
                        // status and puts the error on the timeline.
                        //
                        // A terminal is right HERE and wrong in the fan-in's
                        // own rollback paths. This wake WAS delivered and
                        // consumed, so the next boot's
                        // `refire_unprocessed_child_completions` should skip
                        // this parent rather than re-drive a failed turn.
                        //
                        // Unlike this call's sibling turn-spawning sites, the
                        // wake can also fail BEFORE the turn starts, so the
                        // emit stays. It is anchored on the completion event,
                        // which is the turn's pre-emitted origin, so the gate
                        // drops it when the turn already settled itself.
                        crate::engine::failure_terminator::ensure_failure_terminator_emitted(
                            &engine.event_bus,
                            engine.pool(),
                            cb.parent_thread_id,
                            cb.child_completed_event_id,
                            None,
                            &e.to_string(),
                        )
                        .await;
                    }
                });
            }
        });
    }

    pub fn start_spawn_request_consumer(
        self: &Arc<Self>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<crate::engine::event_bus::SpawnRequest>,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let engine = engine.clone();
                tokio::spawn(async move {
                    use crate::engine::event_bus::SpawnRequest;
                    match req {
                        SpawnRequest::Continue {
                            thread_id,
                            event_id,
                        } => {
                            crate::log_legacy!(
                                "[SpawnConsumer] Continue thread={} event={} — invoking run_direct_agent (--resume + non-empty stdin placeholder)",
                                thread_id,
                                event_id
                            );

                            let (continue_actor, continue_reason): (
                                Option<crate::engine::thread_events::MessageOrigin>,
                                Option<String>,
                            ) = match engine.event_store().get_event_by_id(event_id).await {
                                Ok(Some(row)) => {
                                    let actor = row.payload.get("actor").and_then(|v| {
                                        match serde_json::from_value::<
                                            crate::engine::thread_events::MessageOrigin,
                                        >(v.clone())
                                        {
                                            Ok(o) => Some(o),
                                            Err(e) => {
                                                crate::log_legacy!(
                                                    "[SpawnConsumer] Continue event {} has malformed actor payload: {} — chip will fall back to engine",
                                                    event_id, e
                                                );
                                                None
                                            }
                                        }
                                    });
                                    let reason = row
                                        .payload
                                        .get("reason")
                                        .and_then(|v| v.as_str())
                                        .map(String::from);
                                    (actor, reason)
                                }
                                Ok(None) => (None, None),
                                Err(e) => {
                                    crate::log_legacy!(
                                        "[SpawnConsumer] Failed to load continue event {}: {} — chip will fall back to engine and the resume boundary will be skipped",
                                        event_id, e
                                    );
                                    (None, None)
                                }
                            };

                            let mut claim = loop {
                                match engine.claim_thread_for_spawn(thread_id, None).await {
                                    Ok(Some(claim)) => break Ok(claim),
                                    Err(e) => break Err(e),
                                    Ok(None) => {
                                        let input = engine
                                            .agent_host()
                                            .continue_input_for_reason(
                                                &engine,
                                                thread_id,
                                                continue_reason.as_deref(),
                                            )
                                            .await;
                                        let routed = engine
                                            .route_to_live_run(
                                                thread_id,
                                                crate::engine::AgentUserInput::continuation(
                                                    input, event_id,
                                                ),
                                            )
                                            .await;
                                        if routed {
                                            crate::log_legacy!(
                                                "[SpawnConsumer] Continue thread={} event={} routed to the live run",
                                                thread_id,
                                                event_id
                                            );
                                            return;
                                        }
                                    }
                                }
                            };

                            if claim.is_ok()
                                && crate::engine::continuation_reason::continue_should_open_resume_exchange(
                                    continue_reason.as_deref(),
                                )
                            {
                                engine
                                    .event_bus
                                    .emit_or_log(
                                        crate::engine::event_bus::BusEvent::Thread {
                                            thread_id,
                                            event: crate::engine::thread_events::ThreadEvent::ContinuationStarted {
                                                branch: String::new(),
                                                origin: None,
                                                reason: continue_reason.clone(),
                                                resumed_abort_id: None,
                                            },
                                            meta: crate::engine::thread_events::EventMeta {
                                                channel: Some(
                                                    crate::engine::thread_events::EventChannel::ClaudeCode,
                                                ),
                                                actor: continue_actor.clone(),
                                                ..crate::engine::thread_events::EventMeta::NONE
                                            },
                                        },
                                        "[SpawnConsumer] ContinuationStarted (continue)",
                                    )
                                    .await;
                            }

                            // INVARIANT: Re-attach an in-flight conflict-resolution duty.
                            // A stray-killed merge session's completion hands
                            // off instead of aborting (see
                            // `ConflictResolutionCleanupAction::HandOff`), so
                            // the pending change's `MergeConflictDetected` is
                            // still unpaired — the resumed session must carry
                            // `conflict_change_id` (and run in the merge
                            // worktree), or the merge duty is silently dropped
                            // and the apply the user is watching never
                            // resolves.
                            let conflict_duty = engine
                                .resolve_continue_conflict_duty(
                                    thread_id,
                                    continue_reason.as_deref(),
                                )
                                .await;
                            let conflict_claim = match conflict_duty.as_ref() {
                                Some((change, _)) => {
                                    let claim = engine
                                        .claim_pending_change(change.id)
                                        .await
                                        .unwrap_or_else(|e| {
                                            crate::log_legacy!(
                                                "[SpawnConsumer] Could not claim change {} for its merge duty: {}",
                                                change.id,
                                                e
                                            );
                                            None
                                        });
                                    if claim.is_none() {
                                        crate::log_legacy!(
                                            "[SpawnConsumer] Continue thread={} leaves the merge duty for change {} to the apply that holds it",
                                            thread_id,
                                            change.id
                                        );
                                    }
                                    claim
                                }
                                None => None,
                            };
                            let conflict_duty = conflict_duty.filter(|_| conflict_claim.is_some());
                            let conflict_worktree =
                                conflict_duty.as_ref().map(|(_, pair)| pair.clone());

                            let resume_sid =
                                crate::engine::session_queries::lookup_latest_cc_session_id(
                                    engine.pool(),
                                    thread_id,
                                )
                                .await;
                            let request_id = uuid::Uuid::new_v4();
                            let cancel_token = tokio_util::sync::CancellationToken::new();
                            // INVARIANT: Reason-dependent, and guaranteed non-empty by
                            // `continue_input_for_reason` (see
                            // CONTINUE_RESUME_USER_MESSAGE's doc for the
                            // empty-stdin zombie write-up). An
                            // `answered_after_idle` continuation carries the
                            // answer it was spawned for; every other reason
                            // gets the bare continue message, unless
                            // `resume_input` below replaces it.
                            //
                            // Direct run_direct_agent (bypasses
                            // process_message_with_steps): engine-driven
                            // Claude Code session recovery via `--resume`
                            // against `resume_sid`. The unified router has
                            // no `--resume`/resume_sid concept — chat
                            // rebuilds context from events on every
                            // iteration. Continue is fundamentally a Claude
                            // Code session resurrection, not a new user
                            // message.
                            //
                            // A thread whose agent never engaged gets the prompts
                            // it was never given, for every reason.
                            //
                            // Try again after a start failure delivers the
                            // message the agent never read, with its images.
                            //
                            // The first prompt carries the inputs' ids, so its
                            // read settles them.
                            let undelivered = crate::engine::undelivered_input::resume_input(
                                engine.pool(),
                                thread_id,
                                continue_reason.as_deref(),
                            )
                            .await;
                            let carried_event_ids = undelivered
                                .as_ref()
                                .map_or(&[][..], |u| u.event_ids.as_slice());
                            let continue_images = undelivered
                                .as_ref()
                                .map(|u| engine.resolve_message_image_hashes(&u.image_hashes))
                                .unwrap_or(Ok(None));
                            let continue_input = match &undelivered {
                                Some(u) => u.text.clone(),
                                None => {
                                    engine
                                        .agent_host()
                                        .continue_input_for_reason(
                                            &engine,
                                            thread_id,
                                            continue_reason.as_deref(),
                                        )
                                        .await
                                }
                            };
                            let mut result = match (claim.as_mut(), continue_images.as_ref()) {
                                (_, Err(e)) => Err(e.to_string().into()),
                                (Err(e), _) => Err(e.to_string().into()),
                                (Ok(claim), Ok(images)) => {
                                    engine
                                        .agent_host()
                                        .run_direct_agent(
                                            &engine,
                                            claim,
                                            request_id,
                                            &continue_input,
                                            images.as_deref(),
                                            event_id,
                                            carried_event_ids,
                                            None,
                                            &cancel_token,
                                            conflict_claim.as_ref(),
                                            conflict_worktree.clone(),
                                            None,
                                            None,
                                            resume_sid,
                                            None,
                                            None,
                                            None,
                                        )
                                        .await
                                }
                            };

                            use crate::engine::continuation_reason::ContinueRecovery;
                            let mut retried = false;
                            let err_text =
                                |r: &Result<_, Box<dyn std::error::Error + Send + Sync>>| {
                                    r.as_ref().err().map(|e| e.to_string())
                                };

                            if crate::engine::continuation_reason::continue_recovery(
                                err_text(&result).as_deref(),
                                retried,
                            ) == ContinueRecovery::RetryFresh
                            {
                                crate::log_legacy!(
                                    "[SpawnConsumer] Stale resume on continuation thread={} — retrying with a fresh session",
                                    thread_id
                                );
                                retried = true;
                                let retry_text = match &undelivered {
                                    Some(u) => {
                                        engine
                                            .agent_host()
                                            .resume_failed_input(&engine, thread_id, &u.text)
                                            .await
                                    }
                                    None => {
                                        engine
                                            .agent_host()
                                            .continue_retry_input(
                                                &engine,
                                                thread_id,
                                                continue_reason.as_deref(),
                                            )
                                            .await
                                    }
                                };
                                result = match claim.as_mut() {
                                    Err(e) => Err(e.to_string().into()),
                                    Ok(claim) => {
                                        engine.agent_host().run_direct_agent(
 &engine,
                                                claim,
                                                request_id,
                                                &retry_text,
                                                continue_images
                                                    .as_ref()
                                                    .expect("only a started session can request a fresh retry")
                                                    .as_deref(),
                                                event_id,
                                                carried_event_ids,
                                                None,
                                                &cancel_token,
                                                conflict_claim.as_ref(),
                                                conflict_worktree.clone(),
                                                None,
                                                None,
                                                None,
                                                None,
                                                None,
                                                None,
                                            )
                                            .await
                                    }
                                };
                            }

                            let final_err = err_text(&result);
                            let settle = match crate::engine::continuation_reason::continue_recovery(
                                final_err.as_deref(),
                                retried,
                            ) {
                                ContinueRecovery::Settle => true,
                                ContinueRecovery::Nothing | ContinueRecovery::RetryFresh => false,
                            };
                            // INVARIANT: A turn this continuation owned says why it failed:
                            // a spawn refusal, or the CLI's own exit. Land that as
                            // the turn's failure while the claim still holds, so no
                            // new turn can start under it. A failed claim owned no
                            // turn, and another session may hold the thread. A run
                            // parked on the owner's card is waiting, not failed.
                            if let (true, Err(error), Ok(_)) = (settle, &result, &claim) {
                                crate::engine::failure_terminator::ensure_run_failure_terminator_emitted(
                                    &engine.event_bus,
                                    engine.pool(),
                                    thread_id,
                                    event_id,
                                    Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                                    error.as_ref(),
                                )
                                .await;
                            }
                            drop(claim);
                            if settle {
                                let e = final_err.as_deref().unwrap_or_default();
                                crate::log_legacy!(
                                    "[SpawnConsumer] Continue thread={} event={} failed: {}",
                                    thread_id,
                                    event_id,
                                    e
                                );
                                // INVARIANT: Fail-loud backstop for a re-attached duty:
                                // the resumed session died before its
                                // completion could settle the merge (spawn
                                // failure, DB error, missing merge worktree
                                // state). The hand-off deliberately skipped
                                // the failure emits, so without this the
                                // apply dangles forever — pending change,
                                // open pairing, no toast, an Apply-All batch
                                // member that never resolves. Re-check the
                                // pairing first: run_direct_agent can also
                                // fail AFTER its completion already applied
                                // or aborted, and those paths closed the
                                // pairing themselves.
                                if let Some((change, wt)) = conflict_duty {
                                    let still_open = engine
                                        .changes()
                                        .conflict_pairing_open(thread_id, change.id)
                                        .await
                                        .unwrap_or_else(|e| {
                                            crate::log_legacy!(
                                                "[SpawnConsumer] pairing re-check failed for change {}: {} — leaving duty as-is",
                                                change.id,
                                                e
                                            );
                                            false
                                        });
                                    if still_open {
                                        engine
                                            .close_stranded_conflict_duty(
                                                thread_id,
                                                &change,
                                                "continuation failed before settling the merge",
                                                Some(wt.0.as_path()),
                                            )
                                            .await;
                                    }
                                }

                                // INVARIANT: Zombie-`running` backstop. A continuation is
                                // engine-driven: nothing else is watching it. The
                                // in-memory watchdogs only scan live
                                // `agent_sessions`, and
                                // `settle_orphaned_running_threads`
                                // runs only at boot, so a `running` projection
                                // left behind here survives until the user clicks
                                // Stop. Safe to run for EVERY settling error, not
                                // just the ones known to skip their own terminal:
                                // `settle_stuck_running_thread` re-checks
                                // `running` first, so it no-ops whenever a
                                // terminal already landed.
                                //
                                // Actor: the device that triggered the
                                // continuation (Switch / Continue), so the chip
                                // reads "You" rather than "System". The cause is
                                // `StaleSettle`, NOT `EngineShutdown`, so this can
                                // never be mistaken for a switch teardown by
                                // `switch_was_user_initiated`.
                                let settle_actor = continue_actor.clone().unwrap_or_else(
                                    crate::engine::thread_events::MessageOrigin::system,
                                );
                                match engine.agent_host().settle_unclaimed_running_thread(
 &engine,
                                        thread_id,
                                        Some(settle_actor),
                                        crate::engine::agent_host::SettleTerminal::StuckProjection,
                                    )
                                    .await
                                {
                                    Ok(true) => crate::log_legacy!(
                                        "[SpawnConsumer] Settled thread {} left `running` by a failed continuation",
                                        thread_id
                                    ),
                                    Ok(false) => {}
                                    Err(e) => crate::log_legacy!(
                                        "[SpawnConsumer] Failed to settle thread {} after a failed continuation: {}",
                                        thread_id,
                                        e
                                    ),
                                }
                            }
                            if let Ok(res) = result {
                                if !res.orphaned_injections.is_empty() {
                                    engine
                                        .agent_host()
                                        .process_orphan_chain(
                                            &engine,
                                            thread_id,
                                            res.orphaned_injections,
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                });
            }
            crate::log_legacy!("[SpawnConsumer] channel closed — consumer exiting");
        });
    }

    /// INVARIANT: Try to re-attach an in-flight conflict-resolution duty to a `Continue`
    /// spawn: the pending change whose `MergeConflictDetected` pairing is
    /// still open, plus the worktree the resumed merge runs in — the
    /// temp-worktree shape from the change row when recorded and still on
    /// disk, else the thread's own worktree for the change branch (mirroring
    /// `run_merge_session_tier2`).
    ///
    /// Gated on the continuation being recovery-shaped
    /// (`continue_should_open_resume_exchange`): a recovery resumes the
    /// interrupted merge turn itself, so the duty rides along. An
    /// `answered_after_idle` continuation is a DIFFERENT interaction —
    /// binding a (possibly stranded) open pairing to it would let an
    /// unrelated question-answer turn end `Generated` and silently ff-merge
    /// the change without a fresh Apply click.
    ///
    /// A duty that exists but can no longer be carried (no worktree anywhere)
    /// is closed loudly here rather than dropped — see
    /// `close_stranded_conflict_duty`. "No worktree anywhere" means git said
    /// so. A lookup that could not run leaves the duty parked instead.
    async fn resolve_continue_conflict_duty(
        &self,
        thread_id: Uuid,
        continue_reason: Option<&str>,
    ) -> Option<(crate::core::changes::Change, (PathBuf, String))> {
        if !crate::engine::continuation_reason::continue_should_open_resume_exchange(
            continue_reason,
        ) {
            return None;
        }
        let change = match self
            .changes()
            .pending_conflict_change_for_thread(thread_id)
            .await
        {
            Ok(Some(c)) => c,
            Ok(None) => return None,
            Err(e) => {
                crate::log_legacy!(
                    "[SpawnConsumer] Continue thread={}: conflict-duty lookup failed: {} — resuming without re-attaching the merge",
                    thread_id,
                    e
                );
                return None;
            }
        };
        let recorded = match change.merge_worktree() {
            Some(merge) => {
                let p = PathBuf::from(&merge.path);
                if crate::engine::git_ops::is_live_worktree_at(&p).await {
                    Some((p, merge.temp_branch.clone()))
                } else {
                    None
                }
            }
            None => None,
        };
        let worktree = match recorded {
            Some(pair) => Some(pair),
            None => match crate::engine::git_ops::find_worktree_for_branch(
                std::path::Path::new(&change.repo_root),
                &change.branch_name,
            )
            .await
            {
                crate::engine::git_ops::WorktreeLookup::Found(p) => {
                    Some((p, change.branch_name.clone()))
                }
                crate::engine::git_ops::WorktreeLookup::NotFound => None,
                // INVARIANT: Closing the duty runs `merge --abort`, `worktree remove
                // --force` and `branch -D` on the recorded temp pair, then
                // writes MergeResolutionCleared and ChangeApplyFailed. None of
                // that may ride on a probe that never ran. The duty stays
                // parked for the next boot instead.
                crate::engine::git_ops::WorktreeLookup::Unknown => {
                    crate::log_legacy!(
                        "[SpawnConsumer] Continue thread={} could not locate the worktree for branch {} (git worktree list gave no answer); leaving the conflict duty parked",
                        thread_id,
                        change.branch_name
                    );
                    return None;
                }
            },
        };
        match worktree {
            Some(pair) => {
                crate::log_legacy!(
                    "[SpawnConsumer] Continue thread={} re-attaching conflict resolution for change {} (branch {})",
                    thread_id,
                    change.id,
                    change.branch_name
                );
                Some((change, pair))
            }
            None => {
                self.close_stranded_conflict_duty(
                    thread_id,
                    &change,
                    "merge worktree no longer exists",
                    None,
                )
                .await;
                None
            }
        }
    }

    async fn close_stranded_conflict_duty(
        &self,
        thread_id: Uuid,
        change: &crate::core::changes::Change,
        why: &str,
        merge_worktree: Option<&std::path::Path>,
    ) {
        crate::log_legacy!(
            "[SpawnConsumer] Closing stranded conflict resolution for change {} ({}) — emitting the failure the hand-off deferred",
            change.id,
            why
        );
        let temp_pair = change.merge_worktree();
        if let Some(wt) = merge_worktree.filter(|wt| {
            temp_pair
                .as_ref()
                .is_none_or(|m| std::path::Path::new(&m.path) != *wt)
        }) {
            let _ = crate::engine::git_ops::git_cmd(&["merge", "--abort"], wt).await;
        }
        // INVARIANT: The MergeResolutionCleared below nulls the row's merge columns —
        // the startup stale-merge cleanup keys on them, so any recorded
        // Tier-3 temp state must be cleared NOW or the temp worktree +
        // branch leak with no pointer left anywhere (the completion Abort
        // arm deletes them for the same reason).
        if let Some(merge) = temp_pair {
            let repo = std::path::PathBuf::from(&change.repo_root);
            if let Err(e) = crate::engine::git_ops::clear_temp_merge_state(
                &repo,
                std::path::Path::new(&merge.path),
                &merge.temp_branch,
                &change.branch_name,
                "[SpawnConsumer]",
            )
            .await
            {
                log_legacy!(
                    "[SpawnConsumer] Temp merge state for change {} needs manual cleanup ({}): the merge columns are cleared below, so nothing points at it any more",
                    change.id,
                    e
                );
            }
        }
        let actor = self.pending_apply_actors.take(change.id);
        let tid = change.thread_id.unwrap_or(thread_id);
        self.emit_merge_resolution_cleared(
            tid,
            change.id,
            "[SpawnConsumer] MergeResolutionCleared (stranded duty)",
        )
        .await;
        self.emit_apply_failed(
            tid,
            change.id,
            "Conflict resolution could not resume after recovery, so Lucidos stopped the merge. The change is still pending; try applying again.",
            actor,
        )
        .await;
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn new(
        workspace_path: PathBuf,
        database_url: &str,
        provider_ctx: crate::llm::ProviderBuildContext,
        agent_host: Arc<dyn agent_host::AgentHost>,
        workspace_providers: Box<dyn provider_host::WorkspaceProviders>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::new_with_settlement(
            workspace_path,
            database_url,
            provider_ctx,
            agent_host,
            workspace_providers,
            None,
            &restore_boot::NormalRestoreSettlement,
        )
        .await
    }

    pub async fn new_with_settlement(
        workspace_path: PathBuf,
        database_url: &str,
        provider_ctx: crate::llm::ProviderBuildContext,
        agent_host: Arc<dyn agent_host::AgentHost>,
        workspace_providers: Box<dyn provider_host::WorkspaceProviders>,
        restore_boot: Option<crate::core::snapshot::first_boot::VerifiedRestoreBoot<'_>>,
        settlement: &dyn restore_boot::RestoreSettlement,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        match &restore_boot {
            Some(restore_boot) => restore_boot.validate_binding(&workspace_path, database_url)?,
            None => crate::core::snapshot::boot_guard::ensure_normal_boot(database_url)?,
        }
        Self::new_inner(
            workspace_path,
            database_url,
            provider_ctx,
            agent_host,
            workspace_providers,
            restore_boot,
            settlement,
        )
        .await
    }

    async fn new_inner(
        workspace_path: PathBuf,
        database_url: &str,
        provider_ctx: crate::llm::ProviderBuildContext,
        agent_host: Arc<dyn agent_host::AgentHost>,
        workspace_providers: Box<dyn provider_host::WorkspaceProviders>,
        restore_boot: Option<crate::core::snapshot::first_boot::VerifiedRestoreBoot<'_>>,
        settlement: &dyn restore_boot::RestoreSettlement,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // INVARIANT: Install the subprocess-origin token before anything spawns a
        // subprocess. See `api::actor` module docs.
        crate::core::subprocess_origin::init_agent_origin_secret(Uuid::new_v4().to_string());

        // INVARIANT: Snapshot BEFORE `ArtifactManager::new` runs, because that call
        // git-inits the workspace itself if `.git/` is missing. We use this
        // flag below to decide whether to pin the allocated vite port — only
        // brand-new workspaces should get an auto-pinned `lucidos.toml`.
        let workspace_was_uninitialized = !workspace_path.join(".git").exists();

        let artifact_manager = ArtifactManager::new(workspace_path.clone())?;

        if let Err(e) = crate::core::ensure_workspace_gitignore_entries(&workspace_path) {
            log_legacy!(
                "[Startup] Failed to ensure workspace .gitignore entries: {}",
                e
            );
        }

        match crate::core::ensure_state_dir_excluded_from_file_backup(&workspace_path) {
            Ok(true) => log_legacy!("[Startup] Excluded .lucidos/ from file-level backup"),
            Ok(false) => {}
            Err(e) => log_legacy!(
                "[Startup] Failed to exclude .lucidos/ from file-level backup: {}",
                e
            ),
        }

        let behind_gateway = std::env::var("LUCIDOS_BIND_LOOPBACK")
            .is_ok_and(|v| crate::core::prefs::env_switch_is_on(&v));
        if workspace_was_uninitialized && !behind_gateway {
            match std::env::var("LUCIDOS_API_PORT")
                .ok()
                .and_then(|s| s.parse::<u16>().ok())
            {
                Some(vite_port) if vite_port >= crate::core::DEV_ENGINE_PORT_BASE => {
                    match crate::core::pin_workspace_vite_port(&workspace_path, vite_port) {
                        Ok(true) => log_legacy!(
                            "[Startup] Pinned vite port {} in lucidos.toml",
                            vite_port
                        ),
                        Ok(false) => {}
                        Err(e) => log_legacy!("[Startup] Failed to pin vite port in lucidos.toml: {}", e),
                    }
                }
                Some(vite_port) => log_legacy!(
                    "[Startup] LUCIDOS_API_PORT={} is below the {} minimum scripts/lib/ports.sh accepts, skipping lucidos.toml port pin",
                    vite_port,
                    crate::core::DEV_ENGINE_PORT_BASE
                ),
                None => log_legacy!(
                    "[Startup] LUCIDOS_API_PORT not set or invalid — skipping lucidos.toml port pin"
                ),
            }
        }

        crate::core::migrate_prompts_to_intents(&workspace_path);

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(50)
            .acquire_timeout(std::time::Duration::from_secs(60))
            // INVARIANT: Every workspace's engine shares one Postgres, and each backend
            // costs it ~20 MB, so a burst must not keep its backends open.
            // sqlx hands out the oldest idle connection first, so steady light
            // traffic keeps every one warm and idle_timeout alone never fires.
            // The short max_lifetime retires them, and the pool reopens only
            // what concurrent demand needs.
            .idle_timeout(std::time::Duration::from_secs(60))
            .max_lifetime(std::time::Duration::from_secs(300))
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET hnsw.ef_search = 100")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;

        if let Some(message) = crate::core::migration_marker::refuse_wiped_database(
            &workspace_path,
            database_url,
            &pool,
        )
        .await
        {
            crate::boot_failure::report(&message).await;
            return Err(message.into());
        }

        crate::boot_report::report(crate::boot_report::MIGRATING);
        let migrator = &crate::core::store::MIGRATOR;
        let migrate_started = std::time::Instant::now();
        let migrated = migrator.run(&pool).await;
        log_legacy!(
            "[Startup] Migrations checked in {}ms",
            migrate_started.elapsed().as_millis()
        );
        if let Err(e) = migrated {
            // INVARIANT: Some migration failures are TERMINAL — no respawn fixes them — and
            // the most common is an app DOWNGRADE onto a database a newer Lucidos
            // already migrated (`VersionMissing`). Translate those into something
            // the user can act on and hand it to the gateway BEFORE we exit, or
            // the splash spins forever on the neutral label while the real reason
            // sits in the engine log.
            //
            // `None` means retryable (a dropped connection, a transient IO fault):
            // report nothing, so the supervisor's respawn still gets its chance.
            // See `boot_failure`.
            if let Some(message) = crate::boot_failure::terminal_migration_message(
                &e,
                &crate::boot_failure::applied_versions(&pool).await,
                &crate::boot_failure::embedded_versions(migrator),
                env!("CARGO_PKG_VERSION"),
            ) {
                crate::boot_failure::report(&message).await;
                return Err(message.into());
            }
            return Err(e.into());
        }
        crate::core::snapshot::restore_database::finish_access_after_migrations(&pool).await?;
        crate::core::migration_marker::record_migrated(&workspace_path, database_url, &pool).await;
        match crate::core::secret_store::boot::load_workspace_key(&pool, database_url).await {
            Ok(None) => {}
            Ok(Some(message)) => {
                crate::boot_failure::report(&message).await;
                return Err(message.into());
            }
            Err(e) => {
                if let Some(message) = e.downcast_ref::<sqlx::Error>().and_then(|db| {
                    crate::boot_failure::terminal_database_error_message(
                        db,
                        "the secret check at boot",
                        env!("CARGO_PKG_VERSION"),
                    )
                }) {
                    crate::boot_failure::report(&message).await;
                    return Err(message.into());
                }
                return Err(e);
            }
        }
        crate::core::secret_store::one_home::move_at_boot(&pool).await?;
        crate::core::backup::key::move_legacy_file_at_boot(&workspace_path);
        // INVARIANT: Subprocesses sign in as the agent role, which cannot read a secret
        // column. Without it, boot fails rather than hand out this role.
        let workspace_origin = std::env::var("LUCIDOS_WORKSPACE_ORIGIN").ok();
        crate::core::set_agent_database_url(
            crate::core::agent_role::provision(&pool, database_url, workspace_origin.as_deref())
                .await?,
        );
        // INVARIANT: The boot load in main.rs ran before this migrator. On a fresh database
        // it found no `models` table, and every model would read as unknown.
        crate::llm::model_registry::reload(&provider_ctx.model_registry, &pool).await;
        crate::engine::aux_purpose::seed_not_served(&pool, &provider_ctx.not_served).await;

        let event_store = EventStore::new(pool.clone());

        let (event_bus, parent_callback_rx) = event_bus::EventBus::new(pool.clone());
        // INVARIANT: Restored apply authority must be durably ended before any boot job
        // or subscriber can dispatch work. A failure keeps normal boot closed.
        settlement.settle(&event_bus, restore_boot).await?;
        crate::core::secret_reference::install_notice_bus(&event_bus);
        crate::runtime::agent_sandbox::install_notice_bus(&event_bus);
        // INVARIANT: Recover durable archive jobs under both locks before live HTTP
        // routes or the scheduler can start another archive operation.
        let archive_job_slot = crate::core::snapshot::ArchiveJobSlot::default();
        crate::core::snapshot::service::Session::recover_at_boot(
            database_url,
            crate::core::base_path::workspace_id(),
            archive_job_slot.clone(),
            event_bus.clone(),
        )
        .await?;

        // INVARIANT: One-shot, idempotent: convert legacy base64 image payloads
        // (MessageReceived.user_images, thread_summaries.compose_images) to
        // content-addressed blobs under data/blobs/ + hash-array fields.
        // Run before publishing the live router so readers never see a mixed shape.
        // Re-runs are no-ops once the gate query matches zero rows.
        match crate::core::image_migration::migrate_legacy_image_payloads(&pool, &workspace_path)
            .await
        {
            Ok((events, drafts)) if events > 0 || drafts > 0 => log_legacy!(
                "[ImageMigration] migrated {} event(s) and {} draft(s) to content-addressed blobs",
                events,
                drafts
            ),
            Ok(_) => {}
            Err(e) => log_legacy!("[ImageMigration] failed: {}", e),
        }

        match crate::core::image_described_backfill::backfill_image_described_from_legacy_payload(
            &pool,
            &event_bus,
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => log_legacy!(
                "[ImageDescribedBackfill] backfilled {} ImageDescribed event(s) from legacy MessageReceived rows",
                n
            ),
            Err(e) => log_legacy!("[ImageDescribedBackfill] failed: {}", e),
        }

        let from_events = event_store.backfill_trigger_id_from_events().await?;
        if from_events > 0 {
            log_legacy!(
                "[Engine] Backfilled {} thread_summaries rows: trigger_id NULL → first TriggerStarted event",
                from_events
            );
        }

        let backfilled = event_store.backfill_trigger_id_v5_to_config_id().await?;
        if backfilled > 0 {
            log_legacy!(
                "[Engine] Backfilled {} thread_summaries rows: trigger_id v5-hash → config.id",
                backfilled
            );
        }

        let repo_names = event_store.backfill_repo_names_from_changes().await?;
        if repo_names > 0 {
            log_legacy!(
                "[Engine] Backfilled {} repo_names rows from changes.repo_root (pre-RepositoryAdded repos)",
                repo_names
            );
        }
        let python_runtime = PythonRuntime::new(workspace_path.clone())?;
        let repo_root = git_ops::main_worktree().await;

        let (system_widgets_dir, system_widgets_warning) = crate::core::resolve_system_widgets_dir(
            std::env::var(crate::core::system_widgets::SYSTEM_WIDGETS_DIR.env_var)
                .ok()
                .as_deref(),
            &repo_root,
            crate::runtime::is_packaged(),
        );
        if let Some(warning) = &system_widgets_warning {
            log_legacy!("{}", warning);
        }
        let app_manager =
            Arc::new(AppManager::new(&workspace_path)?.with_system_widgets(system_widgets_dir));

        // INVARIANT: The embedding model that powers vector memory is a multi-hundred-MB
        // HuggingFace download on a cold cache (and a non-trivial ONNX load even
        // when warm), so it must NEVER block boot — a workspace should open
        // immediately regardless of the model. Construction fills the slot EMPTY;
        // `spawn_embedder_load` (from `main.rs`, once the engine is assembled)
        // loads the model in a background task and installs it live once it
        // lands. Until then every embed errors descriptively and consumers
        // degrade (memory tools report it, thread search → text-only, context
        // build skips recall — see `memory::EmbedderSlot`).
        let embedder = crate::memory::embedder_slot::EmbedderSlot::empty();

        let browser_runtime = BrowserRuntime::new(workspace_path.clone(), pool.clone());

        let openai_api_key = crate::llm::openai::openai_api_key(&pool)
            .await
            .map(|(key, _source)| key);

        let memory_index = match PgVectorIndex::new(pool.clone()).await {
            Ok(index) => {
                let count = index.len().await.unwrap_or(0);
                log_legacy!("[Memory] Index initialized (pgvector, {} entries)", count);
                Some(index)
            }
            Err(e) => {
                log_legacy!(
                    "[Memory] Could not initialize index: {}. Memory search disabled.",
                    e
                );
                None
            }
        };

        let user_profile =
            crate::engine::user_profile::UserProfileCache::load_from_workspace(&workspace_path);

        let user_timezone = crate::core::prefs::TIMEZONE
            .stored(&pool)
            .await
            .unwrap_or_default();

        if user_timezone.is_empty() {
            log_legacy!("[Engine] User timezone: not set (LLM will ask)");
        } else {
            log_legacy!("[Engine] User timezone: {}", user_timezone);
        }

        let user_language = crate::core::prefs::LANGUAGE
            .stored(&pool)
            .await
            .unwrap_or_default();

        if user_language.is_empty() {
            log_legacy!("[Engine] User language: not set (will detect from conversation)");
        } else {
            log_legacy!("[Engine] User language: {}", user_language);
        }

        let (system_knowhow_dir, system_knowhow_warning) = crate::core::resolve_system_knowhow_dir(
            std::env::var(crate::core::system_knowhow::SYSTEM_KNOWHOW_DIR.env_var)
                .ok()
                .as_deref(),
            &repo_root,
            crate::runtime::is_packaged(),
        );
        if let Some(warning) = &system_knowhow_warning {
            log_legacy!("{}", warning);
        }

        if crate::paths::has_lucidos_source() {
            let default_repo_root_commit = git_ops::root_commit_sha(&repo_root).await;
            if let Err(e) = crate::core::repositories::RepositoryStore::register(
                &pool,
                &event_bus,
                Self::DEFAULT_REPO_NAME,
                &repo_root.to_string_lossy(),
                None,
                default_repo_root_commit.as_deref(),
                None,
            )
            .await
            {
                log_legacy!(
                    "[Startup] Failed to register default Lucidos repository: {}",
                    e
                );
            }

            // INVARIANT: One-time, marker-guarded: re-point coding-agent threads orphaned by
            // the old random-UUID registry onto the default repo's deterministic
            // id (every lucidos/legacy thread targets the Lucidos source by
            // definition). Must run AFTER `RepositoryStore::register` above so
            // the live row already carries the new id.
            let default_repo_det_id = crate::core::repositories::deterministic_id(
                default_repo_root_commit.as_deref(),
                &repo_root.to_string_lossy(),
            );
            match event_store
                .backfill_cc_repo_id_to_deterministic(default_repo_det_id)
                .await
            {
                Ok(n) if n > 0 => log_legacy!(
                    "[Engine] Backfilled {} thread_summaries rows: cc_repo_id → deterministic repo id {}",
                    n,
                    default_repo_det_id
                ),
                Ok(_) => {}
                Err(e) => log_legacy!("[Startup] cc_repo_id deterministic backfill failed: {}", e),
            }
        } else {
            log_legacy!("[Startup] Packaged build (no source checkout) — skipping Lucidos source repo registration");
        }

        auto_commit_safe_files_if_dirty(&repo_root).await;

        // INVARIANT: Must run before list_pending()-based reconciliation below, so any
        // recovered pending row is visible to the stale-branch discard.
        match event_bus
            .changes_projection()
            .rebuild_missing_from_events()
            .await
        {
            Ok(0) => {}
            Ok(n) => log_legacy!("[Startup] Rebuilt {} missing change row(s) from events", n),
            Err(e) => log_legacy!("[Startup] rebuild_missing_from_events failed: {}", e),
        }

        let stale_merges = match event_bus.changes_projection().with_merge_worktree().await {
            Ok(v) => v,
            Err(e) => {
                log_legacy!(
                    "[Startup] with_merge_worktree: {} — skipping stale merge cleanup",
                    e
                );
                Vec::new()
            }
        };
        // INVARIANT: A restore may keep temp merge trees. If its record cannot be read,
        // keep every tree this boot: an unreadable file must neither authorize
        // deletion nor stop the engine from starting.
        let database_options = pool.connect_options();
        let retained_worktrees = database_options
            .get_database()
            .ok_or_else(|| crate::core::snapshot::BoxError::from("no database name"))
            .and_then(crate::core::snapshot::store::Store::database_root)
            .and_then(|root| {
                crate::core::snapshot::worktree_exemptions::read(&root, &workspace_path)
            });
        let stale_merges = match &retained_worktrees {
            Ok(_) => stale_merges,
            Err(e) => {
                log_legacy!("[Startup] cannot read restore worktree exemptions ({e}); keeping stale merge state");
                Vec::new()
            }
        };
        let retained_worktrees = retained_worktrees.unwrap_or_default();
        for change in stale_merges {
            if let Some(merge) = change.merge_worktree() {
                let change_repo = std::path::PathBuf::from(&change.repo_root);
                let cleared = clear_unretained_temp_merge_state(
                    &change_repo,
                    std::path::Path::new(&merge.path),
                    &merge.temp_branch,
                    &change.branch_name,
                    &retained_worktrees,
                )
                .await;
                if let Some(tid) = change.thread_id {
                    event_bus
                        .emit_or_log(
                            event_bus::BusEvent::Thread {
                                thread_id: tid,
                                event: thread_events::ThreadEvent::MergeResolutionCleared {
                                    change_id: change.id.to_string(),
                                },
                                meta: thread_events::EventMeta::NONE,
                            },
                            "[Startup] MergeResolutionCleared",
                        )
                        .await;
                }
                match cleared {
                    TempMergeCleanup::Retained => {}
                    TempMergeCleanup::Cleared(Ok(())) => log_legacy!(
                        "[Startup] Cleaned up stale merge worktree for change {}",
                        change.id
                    ),
                    TempMergeCleanup::Cleared(Err(e)) => log_legacy!(
                        "[Startup] Stale merge state for change {} needs manual cleanup ({}): the merge columns are cleared, so nothing points at it any more",
                        change.id,
                        e
                    ),
                }
            }
        }

        crate::engine::in_place_merge::end_interrupted_applies(&pool, &event_bus).await;

        // INVARIANT: Surface — but do NOT discard — pending changes whose branches no
        // longer exist. The user resolves these from Review explicitly; the
        // engine never auto-discards on the user's behalf. Apply on a
        // missing-branch row will fail with a useful error and the user
        // can then click Discard. DB error degrades to "skip surfacing this
        // boot"; the projection is still authoritative for Review's UI.
        let stale = match event_bus.changes_projection().list_pending().await {
            Ok(v) => v,
            Err(e) => {
                log_legacy!(
                    "[Startup] list_pending: {} — skipping stale branch surface log",
                    e
                );
                Vec::new()
            }
        };
        for change in stale {
            let change_repo = std::path::PathBuf::from(&change.repo_root);
            let branch_ok = crate::engine::git_ops::git_answer(
                &["rev-parse", "--verify", &change.branch_name],
                &change_repo,
            )
            .await
            .or_unknown(true);
            if !branch_ok {
                log_legacy!(
                    "[Engine] Pending change {} references missing branch {} — left in Review for user to resolve",
                    change.id,
                    change.branch_name
                );
            }
        }

        let mcp_manager =
            crate::mcp::McpManager::new(pool.clone(), event_bus.clone(), workspace_path.clone());

        let user_dir = lucidos_home::path();

        if let Some(ref ud) = user_dir {
            lucidos_home::ensure_storage_root(ud)?;
            crate::core::user_dir::ensure_git_init(ud);
        }

        crate::core::grants::migration::run(user_dir.as_deref(), &workspace_path);

        // INVARIANT: One factory, so every gateway access shares one refresh lock. The
        // runtime credential subscriber rebuilds from the same context, so a
        // hot-swapped provider matches a fresh boot.
        let workspace_providers: Arc<dyn provider_host::WorkspaceProviders> =
            workspace_providers.into();
        let mut provider_build_ctx = provider_ctx;
        workspace_providers.bind_workspace(&pool, &event_bus, &mut provider_build_ctx);
        let crate::llm::ProviderBuildContext {
            vertex_project_id,
            vertex_location,
            vertex_token_cache,
            model_registry,
            ..
        } = provider_build_ctx.clone();
        let provider_build_ctx = Arc::new(provider_build_ctx);

        let crate::llm::ProviderBuildOutcome {
            llm,
            web_search,
            selection,
        } = workspace_providers
            .build_active(Some(&pool), &provider_build_ctx)
            .await?;
        log_legacy!("[Startup] LLM provider installed: {:?}", selection);

        // INVARIANT: Wrap the active provider in the swappable handle BEFORE the struct is
        // assembled, so the credential subscriber below shares the very handle
        // stored on `Self.llm` and its writes are seen by every read site.
        let llm_handle: Arc<std::sync::RwLock<Arc<dyn LlmProvider>>> =
            Arc::new(std::sync::RwLock::new(llm));
        let web_search_handle: Arc<std::sync::RwLock<Arc<crate::llm::WebSearchChain>>> =
            Arc::new(std::sync::RwLock::new(web_search));

        let engine_model_registry = model_registry.clone();

        spawn_vertex_region_subscriber(
            event_bus.subscribe(),
            vertex_location.clone(),
            pool.clone(),
        );
        spawn_models_registry_subscriber(event_bus.subscribe(), model_registry, pool.clone());
        workspace_providers.start_gateway_watchers(&pool, &event_bus, &provider_build_ctx);
        workspace_providers
            .start_gateway_telemetry(&pool, &event_bus, &provider_build_ctx)
            .await;

        if !provider_build_ctx.model_is_mock {
            spawn_provider_config_subscriber(
                event_bus.subscribe(),
                llm_handle.clone(),
                web_search_handle.clone(),
                pool.clone(),
                provider_build_ctx.clone(),
                workspace_providers.clone(),
            );
        }

        // INVARIANT: Build the wasmtime engine ONCE — both `proxy_modules` (compiled
        // here at startup) and `WasmSignerLayer::apply` (instantiated
        // per-request in `proxy.rs`) MUST share this same engine.
        // wasmtime interns `FuncType` identity per-engine, so a separate
        // engine for each side rejects the host imports as "incompatible
        // import type" / panics with "id from different slab".
        let wasm_engine = Arc::new(
            crate::engine::proxy_wasm_signer::build_wasmtime_engine()
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?,
        );

        // INVARIANT: Shared with SchedulerManager AND the Thread Queue (drain consults
        // trigger pause/deletion state), so all three see one registry.
        let trigger_configs: Arc<
            std::sync::RwLock<HashMap<String, crate::triggers::TriggerConfig>>,
        > = Arc::new(std::sync::RwLock::new(HashMap::new()));
        // INVARIANT: Shared with the Thread Queue too: its overflow guard performs a real
        // trigger write, and a lock only the engine held would not serialize it.
        let trigger_write_lock = Arc::new(tokio::sync::Mutex::new(()));
        let shutting_down = Arc::new(AtomicBool::new(false));

        let capacity_policy = thread_queue::ThreadQueue::load_policy(&pool).await;
        let thread_queue_manager = Arc::new(thread_queue::ThreadQueue::new(
            pool.clone(),
            event_bus.clone(),
            trigger_configs.clone(),
            workspace_path.clone(),
            trigger_write_lock.clone(),
            capacity_policy,
            shutting_down.clone(),
        ));

        let version_tracking = crate::engine::engine_version::VersionTracking::from_env();
        if version_tracking == crate::engine::engine_version::VersionTracking::PinnedToBuild {
            crate::log_legacy!(
                "[Rebuild] engine version pinned to {} ({}): newer commits neither rebuild nor announce this engine",
                crate::build_identity().id,
                crate::engine::engine_version::PIN_ENGINE_VERSION_ENV
            );
        }

        Ok(Self {
            agent_host,
            artifact_manager,
            event_store,
            python_runtime,
            browser_runtime,
            app_manager,
            // INVARIANT: The swappable provider handle, shared with the credential
            // subscriber spawned above (built before this struct so both sides
            // hold the same lock).
            llm: llm_handle,
            web_search: web_search_handle,
            embedder,
            memory_index,
            vertex_project_id,
            vertex_location,
            vertex_token_cache,
            model_registry: engine_model_registry,
            provider_context: provider_build_ctx,
            workspace_providers,
            openai_api_key,
            summarizing_threads: Default::default(),
            rebuilding_memory: AtomicBool::new(false),
            cancel_rebuild: AtomicBool::new(false),
            shutting_down,
            archive_job_slot,
            database_health: Default::default(),
            build_state: std::sync::RwLock::new(crate::engine::engine_version::BuildState::Idle),
            version_tracking,
            update_check: std::sync::Mutex::new(Default::default()),
            source_behind_cache: std::sync::Mutex::new(Default::default()),
            disk_direction_cache: std::sync::Mutex::new(Default::default()),
            pending_commits_cache: std::sync::Mutex::new(Default::default()),
            head_sha_cache: std::sync::Mutex::new(Default::default()),
            self_heal_state: std::sync::Mutex::new(Default::default()),
            background_build: Default::default(),
            engine_build_runner: Box::new(
                crate::engine::engine_version::UnregisteredEngineBuildRunner,
            ),
            build_process_group: std::sync::atomic::AtomicU32::new(0),
            served_frontend: std::sync::OnceLock::new(),
            api_router: std::sync::OnceLock::new(),
            activated_frontend_commit: std::sync::Mutex::new(None),
            served_commit_write_done: std::sync::Mutex::new(None),
            served_frontend_source: std::sync::OnceLock::new(),
            frontend_refresh_generation: std::sync::atomic::AtomicU64::new(0),
            frontend_refresh_task: std::sync::Mutex::new(None),
            frontend_refresh_started: std::sync::Mutex::new(None),
            frontend_build_watch_stopped: std::sync::Mutex::new(None),
            frontend_worktree_pin_warned: std::sync::atomic::AtomicBool::new(false),
            summary_tree: Default::default(),
            frontend_preview: tokio::sync::Mutex::new(None),
            frontend_preview_lifecycle: tokio::sync::Mutex::new(()),
            restart_actor: std::sync::Mutex::new(None),
            teardown_actor: std::sync::Mutex::new(None),
            teardown_handover: Default::default(),
            pending_switch_resumes: std::sync::Mutex::new(Vec::new()),
            active_threads: Arc::new(std::sync::Mutex::new(HashMap::new())),
            thread_completion: Arc::new(std::sync::Mutex::new(HashMap::new())),
            cc_commands_cache: tokio::sync::RwLock::new(Self::load_cc_commands_cache(
                &workspace_path,
            )),
            proxy_modules: {
                migrate_legacy_auth_modules_dir(&workspace_path);
                let dir = workspace_path.join("data/auth-modules");
                let modules =
                    match crate::engine::proxy_wasm_signer::load_wasm_modules(&dir, &wasm_engine) {
                        Ok(m) => {
                            if !m.is_empty() {
                                log_legacy!(
                                    "[Startup] loaded {} WASM auth module(s) from {}",
                                    m.len(),
                                    dir.display()
                                );
                            }
                            m
                        }
                        Err(e) => {
                            log_legacy!(
                                "[Startup] WASM auth module load failed at {}: {} \
                                 (proceeding with no signer modules)",
                                dir.display(),
                                e
                            );
                            std::collections::HashMap::new()
                        }
                    };
                Arc::new(tokio::sync::RwLock::new(modules))
            },
            workspace_path,
            repo_root: repo_root.clone(),
            user_dir,
            system_knowhow_dir,
            user_profile,
            user_timezone: tokio::sync::RwLock::new(user_timezone),
            user_language: tokio::sync::RwLock::new(user_language),
            input_order: event_bus.input_order.clone(),
            event_bus: {
                // INVARIANT: parent_callback_rx is stored temporarily in a thread-local;
                // it's extracted and wired up after Arc::new(engine) via start_parent_callback_listener().
                PARENT_CALLBACK_RX.with(|cell| cell.borrow_mut().replace(parent_callback_rx));
                event_bus
            },
            presence_tracker: crate::engine::presence_tracker::PresenceTracker::new(),
            sse_connections: crate::engine::sse_connections::SseConnectionCounter::new(),
            voice_sessions: crate::voice::registry::LiveVoiceSessions::new(),
            threads_being_named: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            pool,
            proxy_token_cache: Arc::new(
                lucidos_core::engine::proxy_token_cache::ProxyTokenCache::new(),
            ),
            wasm_engine,
            pending_captures: Arc::default(),
            pending_widget_fits: Arc::default(),
            widget_fit_sessions: Arc::default(),
            event_hook_catalog: std::sync::OnceLock::new(),
            pending_installs: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            pending_uninstalls: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            frontend_origin: std::sync::Mutex::new(None),
            agent_sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            sign_in_contexts: Default::default(),
            sent_device_contexts: Arc::default(),
            apply_claims: Arc::default(),
            view_snapshots: Default::default(),
            find_cache: Default::default(),
            session_spawn_gate: Default::default(),
            agent_runtimes: {
                let mut m: HashMap<CodingAgent, Arc<dyn AgentRuntime>> = HashMap::new();
                m.insert(CodingAgent::ClaudeCode, Arc::new(ClaudeCodeRuntime));
                m.insert(CodingAgent::Codex, Arc::new(CodexRuntime));
                m
            },
            live_waits: Arc::new(crate::engine::event_wait::LiveWaits::new()),
            keep_alive_beats: Default::default(),
            keep_alive_durations: Default::default(),
            keep_alive_re_estimates: Default::default(),
            remembered_requests: Default::default(),
            keep_alive_status_writes: Default::default(),
            wait_reentry_tx: {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                crate::engine::WAIT_REENTRY_RX.with(|cell| cell.borrow_mut().replace(rx));
                tx
            },
            cc_system_prompts: std::sync::Mutex::new(HashMap::new()),
            pending_app_spawn: std::sync::Mutex::new(HashMap::new()),
            accepted_messages: Default::default(),
            turn_start_gate: Default::default(),
            cc_startup_semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
            mcp_manager,
            pending_cc_permission: Arc::new(std::sync::Mutex::new(
                cc_permission::PermissionState::default(),
            )),
            pending_command_permission: Arc::new(std::sync::Mutex::new(
                cc_permission::PermissionState::default(),
            )),
            pending_mcp_permission: Arc::new(std::sync::Mutex::new(
                cc_permission::PermissionState::default(),
            )),
            question_wait_registry: cc_question_wait::QuestionWaitRegistry::new(),
            pending_apply_actors: pending_apply_actors::PendingApplyActors::default(),
            apply_all_batches: Arc::new(tokio::sync::Mutex::new(
                apply_all_batches::ApplyAllRegistry::default(),
            )),
            apply_estimates: Arc::default(),
            apply_all_drive_tx: {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                APPLY_ALL_DRIVE_RX.with(|cell| cell.borrow_mut().replace(rx));
                tx
            },
            landing_lane_tx: {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                crate::engine::LANDING_LANE_RX.with(|cell| cell.borrow_mut().replace(rx));
                tx
            },
            landing_setting_gate: tokio::sync::Mutex::new(()),
            landing_lane_snapshot: std::sync::RwLock::default(),
            follow_main_nudge: tokio::sync::Notify::new(),
            main_check_tx: {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                crate::engine::MAIN_CHECK_RX.with(|cell| cell.borrow_mut().replace(rx));
                tx
            },
            main_check_snapshot: std::sync::RwLock::default(),
            main_check_runner: Arc::default(),
            standing_applies: Arc::new(crate::engine::standing_apply::ArmedThreads::default()),
            self_arc: std::sync::OnceLock::new(),
            trigger_configs,
            trigger_groups: Arc::default(),
            trigger_group_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            trigger_write_lock,
            bash_background: crate::engine::bash_background::BackgroundBashRegistry::new(),
            terminals: crate::engine::terminal::TerminalRegistry::new(),
            thread_queue: thread_queue_manager,
        })
    }

    fn load_cc_commands_cache(workspace: &std::path::Path) -> HashMap<String, CcCommandsInfo> {
        let path = workspace.join(".lucidos/cc-commands.json");
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                log_legacy!("[ClaudeCode] Failed to parse CC commands cache: {}", e);
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        }
    }
}
enum TempMergeCleanup {
    Retained,
    Cleared(Result<(), String>),
}

async fn clear_unretained_temp_merge_state(
    repo: &std::path::Path,
    worktree: &std::path::Path,
    temp_branch: &str,
    change_branch: &str,
    retained: &std::collections::BTreeSet<std::path::PathBuf>,
) -> TempMergeCleanup {
    if retained.iter().any(|path| worktree.starts_with(path)) {
        log_legacy!("[Startup] Preserving temp merge worktree retained by snapshot restore");
        return TempMergeCleanup::Retained;
    }
    TempMergeCleanup::Cleared(
        crate::engine::git_ops::clear_temp_merge_state(
            repo,
            worktree,
            temp_branch,
            change_branch,
            "[Startup]",
        )
        .await,
    )
}

#[cfg(test)]
mod restore_cleanup_tests {
    use super::*;

    #[tokio::test]
    async fn startup_keeps_retained_merge_tree_and_branch_but_cleans_unretained_state() {
        use crate::engine::git_ops::git_ran_ok;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git_ran_ok(&["init", "-b", "main"], root).await.unwrap();
        git_ran_ok(
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
            root,
        )
        .await
        .unwrap();
        let tree = root.join("merge-tree");
        let path = tree.to_str().unwrap();
        git_ran_ok(&["worktree", "add", "-b", "merge-temp", path], root)
            .await
            .unwrap();
        let retained = std::collections::BTreeSet::from([tree.clone()]);
        assert!(matches!(
            clear_unretained_temp_merge_state(root, &tree, "merge-temp", "main", &retained).await,
            TempMergeCleanup::Retained
        ));
        assert!(tree.is_dir());
        git_ran_ok(&["rev-parse", "--verify", "merge-temp"], root)
            .await
            .unwrap();
        assert!(matches!(
            clear_unretained_temp_merge_state(
                root,
                &tree,
                "merge-temp",
                "main",
                &Default::default()
            )
            .await,
            TempMergeCleanup::Cleared(Ok(()))
        ));
        assert!(!tree.exists());
    }
}
