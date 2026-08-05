//! Engine construction & startup wiring (new, spawn consumers, watchdog, cc-commands cache).
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.

use super::super::*;

impl LucidosEngine {
    pub(crate) const DEFAULT_REPO_NAME: &'static str = "Lucidos";

    /// Keeps `agent_sessions` and `AgentSession` `pub(crate)` while letting
    /// `main.rs` wire `WorktreeCleanup::spawn` from the bin crate.
    pub fn worktree_cleanup_active_threads(&self) -> Arc<dyn worktree_cleanup::ActiveThreads> {
        Arc::new(worktree_cleanup::AgentSessionsActiveThreads::new(
            self.agent_sessions.clone(),
        ))
    }

    /// Floor-level watchdog that scans `agent_sessions` from outside any
    /// per-thread `select!`. See `agent_session::external_watchdog` for
    /// the failure mode it catches that the in-loop watchdog cannot.
    pub fn spawn_external_watchdog(&self) -> tokio::task::JoinHandle<()> {
        agent_session::external_watchdog::ExternalWatchdog::new(
            self.agent_sessions.clone(),
            Arc::new(self.event_bus.clone()),
            self.pool.clone(),
            agent_session::external_watchdog::EXTERNAL_WATCHDOG_LIMIT_MS,
            agent_session::WATCHDOG_HUNG_TOOL_CEILING_MS,
        )
        .spawn()
    }

    /// Start the parent callback listener. Must be called after Arc::new(engine).
    /// Takes the mpsc receiver that was stashed during construction and spawns
    /// a task that resumes parent threads when child threads complete.
    pub fn start_parent_callback_listener(self: &Arc<Self>) {
        let rx = PARENT_CALLBACK_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            crate::log!("[FanOut] No parent callback receiver found — listener not started");
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(cb) = rx.recv().await {
                let engine = engine.clone();
                tokio::spawn(async move {
                    crate::log!(
                        "[FanOut] Child {} completed, notifying parent {}",
                        cb.child_thread_id,
                        cb.parent_thread_id
                    );
                    if let Err(e) = engine
                        .notify_parent_of_child_completion(
                            cb.parent_thread_id,
                            cb.child_thread_id,
                            cb.child_completed_event_id,
                            cb.parent_is_coding_agent,
                        )
                        .await
                    {
                        crate::log!(
                            "[FanOut] Failed to notify parent {} of child {}: {}",
                            cb.parent_thread_id,
                            cb.child_thread_id,
                            e
                        );
                    }
                });
            }
        });
    }

    /// Drain the spawn dispatcher's outbound channel and actuate each
    /// [`SpawnRequest`] against `run_direct_agent`. Started once at engine
    /// boot, after `SpawnDispatcher::spawn(..)` returns the receiver end.
    ///
    /// Today only `SpawnRequest::Continue` flows through this channel — the
    /// chat HTTP handler still owns the spawn for `MessageReceived` (see
    /// `spawn_dispatcher` module docs). For a continue request we re-enter
    /// `run_direct_agent` with empty input so CC reconnects via `--resume`
    /// against the existing session id; CC then sees its prior interrupt
    /// state and continues from there. Phase 5.3 owns enriching this with
    /// the real recovery payload (worktree path, branch, system prompt).
    pub fn start_spawn_request_consumer(
        self: &Arc<Self>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<crate::engine::spawn_dispatcher::SpawnRequest>,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let engine = engine.clone();
                tokio::spawn(async move {
                    use crate::engine::spawn_dispatcher::SpawnRequest;
                    match req {
                        SpawnRequest::Continue {
                            thread_id,
                            event_id,
                        } => {
                            crate::log!(
                                "[SpawnConsumer] Continue thread={} event={} — invoking run_direct_agent (--resume + non-empty stdin placeholder)",
                                thread_id,
                                event_id
                            );

                            // Load the originating ContinuationRequested's actor +
                            // reason. The actor stamps the resume boundary so
                            // it reads "You restarted" rather than "⚙ System".
                            // The reason gates the boundary itself: only the
                            // user-clicked-continue path opens a "Resumed
                            // after engine restart" exchange — an
                            // `answered_after_idle` continuation belongs
                            // inside the existing AskUserQuestion exchange.
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
                                                crate::log!(
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
                                    crate::log!(
                                        "[SpawnConsumer] Failed to load continue event {}: {} — chip will fall back to engine and the resume boundary will be skipped",
                                        event_id, e
                                    );
                                    (None, None)
                                }
                            };

                            // ContinuationStarted opens the resume exchange in
                            // the timeline before any CC text streams. CC's
                            // own `--resume` system carries the prior context
                            // to the model, so no engine note (unlike the
                            // chat/rerun path). Skipped for non-recovery
                            // continuations (answered_after_idle) so the
                            // follow-up CC events fold into the existing
                            // AskUserQuestion exchange instead of being
                            // mislabeled "Resumed after engine restart".
                            if crate::engine::agent_recovery::continue_should_open_resume_exchange(
                                continue_reason.as_deref(),
                            ) {
                                engine
                                    .event_bus
                                    .emit_or_log(
                                        crate::engine::event_bus::BusEvent::Thread {
                                            thread_id,
                                            event: crate::engine::thread_events::ThreadEvent::ContinuationStarted {
                                                branch: String::new(),
                                                origin: None,
                                                // Forward the originating reason so the
                                                // timeline labels the resume honestly:
                                                // user_clicked_continue = real restart
                                                // resume; auto_recovery_after_hang = a
                                                // local hang/stray-signal interrupt, NOT
                                                // a restart.
                                                reason: continue_reason.clone(),
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

                            // Re-attach an in-flight conflict-resolution duty.
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
                            let conflict_change_id = conflict_duty.as_ref().map(|(c, _)| c.id);
                            let conflict_worktree =
                                conflict_duty.as_ref().map(|(_, pair)| pair.clone());

                            // Resolve the latest Claude Code session id from the events
                            // table so `--resume` lands on the prior conversation.
                            let resume_sid =
                                crate::engine::agent_session::lookup_latest_cc_session_id(
                                    engine.pool(),
                                    thread_id,
                                )
                                .await;
                            let request_id = uuid::Uuid::new_v4();
                            let cancel_token = tokio_util::sync::CancellationToken::new();
                            // Must stay non-empty — see CONTINUE_RESUME_USER_MESSAGE
                            // doc for the empty-stdin zombie write-up.
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
                            let mut result = engine
                                .run_direct_agent(
                                    request_id,
                                    thread_id,
                                    crate::engine::agent_recovery::CONTINUE_RESUME_USER_MESSAGE,
                                    None,
                                    event_id,
                                    None,
                                    &cancel_token,
                                    conflict_change_id,
                                    conflict_worktree.clone(),
                                    None,
                                    None,
                                    resume_sid,
                                    None,
                                    None,
                                    None,
                                    None,
                                )
                                .await;

                            // What to do next is decided by the pure
                            // `continue_recovery` (see its doc for why the retry
                            // is mandatory and one-shot); this block only
                            // actuates it.
                            use crate::engine::agent_recovery::ContinueRecovery;
                            let mut retried = false;
                            let err_text =
                                |r: &Result<_, Box<dyn std::error::Error + Send + Sync>>| {
                                    r.as_ref().err().map(|e| e.to_string())
                                };

                            if crate::engine::agent_recovery::continue_recovery(
                                err_text(&result).as_deref(),
                                retried,
                            ) == ContinueRecovery::RetryFresh
                            {
                                crate::log!(
                                    "[SpawnConsumer] Stale resume on continuation thread={} — retrying with a fresh session",
                                    thread_id
                                );
                                retried = true;
                                let retry_text =
                                    crate::engine::agent_recovery::continue_retry_input(
                                        engine.pool(),
                                        thread_id,
                                    )
                                    .await;
                                result = engine
                                    .run_direct_agent(
                                        request_id,
                                        thread_id,
                                        &retry_text,
                                        None,
                                        event_id,
                                        None,
                                        &cancel_token,
                                        conflict_change_id,
                                        conflict_worktree.clone(),
                                        None,
                                        None,
                                        // No sid — the one we had is dead.
                                        None,
                                        None,
                                        None,
                                        None,
                                        None,
                                    )
                                    .await;
                            }

                            let final_err = err_text(&result);
                            if crate::engine::agent_recovery::continue_recovery(
                                final_err.as_deref(),
                                retried,
                            ) == ContinueRecovery::Settle
                            {
                                let e = final_err.as_deref().unwrap_or_default();
                                crate::log!(
                                    "[SpawnConsumer] Continue thread={} event={} failed: {}",
                                    thread_id,
                                    event_id,
                                    e
                                );
                                // Fail-loud backstop for a re-attached duty:
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
                                            crate::log!(
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

                                // Zombie-`running` backstop. A continuation is
                                // engine-driven: nothing else is watching it. The
                                // in-memory watchdogs only scan live
                                // `agent_sessions`, and
                                // `settle_orphaned_running_coding_agent_threads`
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
                                // reads "You" rather than "⚙ System". The cause is
                                // `StaleSettle`, NOT `EngineShutdown`, so this can
                                // never be mistaken for a switch teardown by
                                // `switch_was_user_initiated`.
                                let settle_actor = continue_actor.clone().unwrap_or_else(
                                    crate::engine::thread_events::MessageOrigin::system,
                                );
                                match crate::engine::claude_code::settle_stuck_running_thread(
                                    engine.pool(),
                                    &engine.event_bus,
                                    thread_id,
                                    Some(settle_actor),
                                )
                                .await
                                {
                                    Ok(true) => crate::log!(
                                        "[SpawnConsumer] Settled thread {} left `running` by a failed continuation",
                                        thread_id
                                    ),
                                    Ok(false) => {}
                                    Err(e) => crate::log!(
                                        "[SpawnConsumer] Failed to settle thread {} after a failed continuation: {}",
                                        thread_id,
                                        e
                                    ),
                                }
                            }
                        }
                    }
                });
            }
            crate::log!("[SpawnConsumer] channel closed — consumer exiting");
        });
    }

    /// Try to re-attach an in-flight conflict-resolution duty to a `Continue`
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
    /// `close_stranded_conflict_duty`.
    async fn resolve_continue_conflict_duty(
        &self,
        thread_id: Uuid,
        continue_reason: Option<&str>,
    ) -> Option<(crate::core::changes::Change, (PathBuf, String))> {
        if !crate::engine::agent_recovery::continue_should_open_resume_exchange(continue_reason) {
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
                crate::log!(
                    "[SpawnConsumer] Continue thread={}: conflict-duty lookup failed: {} — resuming without re-attaching the merge",
                    thread_id,
                    e
                );
                return None;
            }
        };
        // Temp-worktree shape (Tier 3) only when the recorded path is still a
        // LIVE worktree (git-aware check, not a bare is_dir — a pruned admin
        // entry with the directory left behind would hand CC a cwd whose git
        // commands resolve to the enclosing repo). A dead one falls through
        // to the branch lookup instead.
        let recorded = match (&change.merge_worktree_path, &change.merge_temp_branch) {
            (Some(wt), Some(tb)) => {
                let p = PathBuf::from(wt);
                if crate::engine::git_ops::is_live_worktree_at(&p).await {
                    Some((p, tb.clone()))
                } else {
                    None
                }
            }
            _ => None,
        };
        let worktree = match recorded {
            Some(pair) => Some(pair),
            None => crate::engine::git_ops::find_worktree_for_branch(
                std::path::Path::new(&change.repo_root),
                &change.branch_name,
            )
            .await
            .map(|p| (p, change.branch_name.clone())),
        };
        match worktree {
            Some(pair) => {
                crate::log!(
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

    /// Fail-loud backstop for a conflict-resolution duty that can no longer
    /// be carried by a continuation: abort any in-progress merge, then emit
    /// the closing pair (`MergeResolutionCleared` + `ChangeApplyFailed`) the
    /// hand-off deliberately deferred — stamped with the parked apply actor —
    /// so the apply resolves with a visible failure instead of dangling as an
    /// eternally-"applying" pending change.
    async fn close_stranded_conflict_duty(
        &self,
        thread_id: Uuid,
        change: &crate::core::changes::Change,
        why: &str,
        merge_worktree: Option<&std::path::Path>,
    ) {
        crate::log!(
            "[SpawnConsumer] Closing stranded conflict resolution for change {} ({}) — emitting the failure the hand-off deferred",
            change.id,
            why
        );
        if let Some(wt) = merge_worktree {
            let _ = crate::engine::git_ops::git_cmd(&["merge", "--abort"], wt).await;
        }
        // The MergeResolutionCleared below nulls the row's merge columns —
        // the startup stale-merge cleanup keys on them, so any recorded
        // Tier-3 temp state must be removed NOW or the temp worktree +
        // branch leak with no pointer left anywhere (the completion Abort
        // arm deletes them for the same reason). These delete only what the
        // merge attempt created (the temp pair recorded by
        // MergeResolutionStarted — never the thread worktree/branch), and
        // every outcome is logged so a refused deletion is triageable
        // (rust.md failure-path-cleanup rule). The extra merge --abort is a
        // harmless no-op when this is the same path as `merge_worktree`.
        if let (Some(wt), Some(tb)) = (&change.merge_worktree_path, &change.merge_temp_branch) {
            let repo = std::path::Path::new(&change.repo_root);
            let _ =
                crate::engine::git_ops::git_cmd(&["merge", "--abort"], std::path::Path::new(wt))
                    .await;
            match crate::engine::git_ops::git_cmd(&["worktree", "remove", "--force", wt], repo)
                .await
            {
                Ok(_) => crate::log!(
                    "[SpawnConsumer] Removed stranded temp merge worktree {}",
                    wt
                ),
                Err(e) => crate::log!(
                    "[SpawnConsumer] Failed to remove stranded temp merge worktree {}: {} — needs manual cleanup (merge columns are cleared below)",
                    wt,
                    e
                ),
            }
            match crate::engine::git_ops::git_cmd(&["branch", "-D", tb], repo).await {
                Ok(_) => crate::log!("[SpawnConsumer] Deleted stranded temp branch {}", tb),
                Err(e) => crate::log!(
                    "[SpawnConsumer] Failed to delete stranded temp branch {}: {}",
                    tb,
                    e
                ),
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
            "Conflict resolution could not resume after recovery — merge aborted. The change is still pending; try applying again.",
            actor,
        )
        .await;
    }

    // Boot wiring with ONE call site (`main.rs`), and no two parameters share a
    // type — so the argument-swap mistake this lint guards against is a compile
    // error here, not a runtime bug. Bundling them into a params struct would
    // buy nothing the type checker isn't already enforcing.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        workspace_path: PathBuf,
        database_url: &str,
        llm: Arc<dyn LlmProvider>,
        web_search: Arc<crate::llm::WebSearchChain>,
        vertex_project_id: String,
        vertex_location: crate::llm::vertex::LocationHandle,
        vertex_token_cache: Option<crate::llm::vertex::TokenCache>,
        model_registry: crate::llm::model_registry::ModelRegistry,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Install the subprocess-origin token before anything spawns a
        // subprocess. See `api::actor` module docs.
        crate::api::actor::init_agent_origin_secret(Uuid::new_v4().to_string());

        // Snapshot BEFORE `ArtifactManager::new` runs, because that call
        // git-inits the workspace itself if `.git/` is missing. We use this
        // flag below to decide whether to pin the allocated vite port — only
        // brand-new workspaces should get an auto-pinned `lucidos.toml`.
        let workspace_was_uninitialized = !workspace_path.join(".git").exists();

        let artifact_manager = ArtifactManager::new(workspace_path.clone())?;

        // Ensure .gitignore contains every engine-managed entry. Idempotent:
        // creates the file on first boot, appends new entries (e.g. data/blobs/)
        // on existing workspaces, no-ops once everything is already in place.
        if let Err(e) = crate::core::ensure_workspace_gitignore_entries(&workspace_path) {
            log!(
                "[Startup] Failed to ensure workspace .gitignore entries: {}",
                e
            );
        }

        // First-ever boot for this workspace: pin the allocated vite port to
        // `lucidos.toml` and commit it. Means the workspace never goes through
        // an untracked-and-dirty state, and the port survives any future
        // `~/.lucidos/port-registry` drift caused by sibling workspaces
        // collision-walking past it.
        //
        // Tauri / Docker installs run with `LUCIDOS_API_PORT` unset (the
        // engine binds the default 3000 internally and there is no
        // shell-driven port allocation), so the `None` arm fires and the
        // workspace stays unpinned — that's intentional, `lucidos.toml` is
        // a dev-mode concept that `scripts/lib/ports.sh` consumes.
        //
        // The 5173 minimum mirrors `_validate_vite_port` in `ports.sh`
        // (a sub-5173 value yields negative API/PG offsets). Writing a
        // value the script will later reject would self-inflict a
        // bootblock on the next dev-mode launch.
        // A PACKAGED gateway engine binds a LOOPBACK port (LUCIDOS_BIND_LOOPBACK=1)
        // that is NOT the user-facing vite port — pinning it into lucidos.toml
        // would poison the next `scripts/lib/ports.sh` allocation (it reads
        // lucidos.toml as the vite pin), so skip it. A DEV gateway engine (ADR
        // 0014 "Dev runtime topology") binds the user-facing vite port DIRECTLY
        // and does NOT set LUCIDOS_BIND_LOOPBACK, so `behind_gateway` is false and
        // it pins the real port — correct, that IS the workspace's vite port.
        let behind_gateway = std::env::var("LUCIDOS_BIND_LOOPBACK")
            .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        if workspace_was_uninitialized && !behind_gateway {
            match std::env::var("LUCIDOS_API_PORT")
                .ok()
                .and_then(|s| s.parse::<u16>().ok())
            {
                Some(vite_port) if vite_port >= 5173 => {
                    match crate::core::pin_workspace_vite_port(&workspace_path, vite_port) {
                        Ok(true) => log!(
                            "[Startup] Pinned vite port {} in lucidos.toml",
                            vite_port
                        ),
                        Ok(false) => {}
                        Err(e) => log!("[Startup] Failed to pin vite port in lucidos.toml: {}", e),
                    }
                }
                Some(vite_port) => log!(
                    "[Startup] LUCIDOS_API_PORT={} is below the 5173 minimum scripts/lib/ports.sh accepts — skipping lucidos.toml port pin",
                    vite_port
                ),
                None => log!(
                    "[Startup] LUCIDOS_API_PORT not set or invalid — skipping lucidos.toml port pin"
                ),
            }
        }

        // Migrate legacy prompts/ directories to intents/ (idempotent)
        crate::core::migrate_prompts_to_intents(&workspace_path);

        // Single shared connection pool for the entire engine
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(50)
            .acquire_timeout(std::time::Duration::from_secs(60))
            .idle_timeout(std::time::Duration::from_secs(600))
            .max_lifetime(std::time::Duration::from_secs(1800))
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    // Tune HNSW search: higher ef_search = better recall at slight latency cost
                    sqlx::query("SET hnsw.ef_search = 100")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;

        // Run sqlx migrations before any schema init calls
        crate::boot_report::report(crate::boot_report::MIGRATING);
        let migrator = sqlx::migrate!();
        if let Err(e) = migrator.run(&pool).await {
            // Some migration failures are TERMINAL — no respawn fixes them — and
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
                &crate::boot_failure::embedded_versions(&migrator),
                env!("CARGO_PKG_VERSION"),
            ) {
                crate::boot_failure::report(&message).await;
            }
            return Err(e.into());
        }

        let event_store = EventStore::new(pool.clone());
        event_store.init_schema().await?;

        // EventBus must exist before `image_described_backfill` runs so the
        // backfill can route through `replay_historical_event` instead of
        // bypassing the bus.
        let (event_bus, parent_callback_rx) = event_bus::EventBus::new(pool.clone());

        // One-shot, idempotent: convert legacy base64 image payloads
        // (MessageReceived.user_images, thread_summaries.compose_images) to
        // content-addressed blobs under data/blobs/ + hash-array fields.
        // Synchronous before HTTP bind so readers never see a mixed shape.
        // Re-runs are no-ops once the gate query matches zero rows.
        match crate::core::image_migration::migrate_legacy_image_payloads(&pool, &workspace_path)
            .await
        {
            Ok((events, drafts)) if events > 0 || drafts > 0 => log!(
                "[ImageMigration] migrated {} event(s) and {} draft(s) to content-addressed blobs",
                events,
                drafts
            ),
            Ok(_) => {}
            Err(e) => log!("[ImageMigration] failed: {}", e),
        }

        // One-shot, idempotent: emit `ImageDescribed` events for legacy
        // `MessageReceived` rows that still carry the deprecated
        // `image_description` payload field. Post-refactor, the agentic
        // loop emits `ImageDescribed` directly via EventBus; this catches
        // up old rows so consumers can read through the new path uniformly.
        // Stable v5 ids derived from `(source_event_id, hash)` — re-runs
        // collide on the primary key and `ON CONFLICT DO NOTHING` skips.
        match crate::core::image_described_backfill::backfill_image_described_from_legacy_payload(
            &pool,
            &event_bus,
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => log!(
                "[ImageDescribedBackfill] backfilled {} ImageDescribed event(s) from legacy MessageReceived rows",
                n
            ),
            Err(e) => log!("[ImageDescribedBackfill] failed: {}", e),
        }

        // One-shot, idempotent: the SQL backfill in migration
        // 20260429214800 only read `payload->>'trigger_id'` and missed legacy
        // `task_id` events, leaving `thread_summaries.trigger_id` NULL on every
        // pre-rename trigger thread. Recover from the events table first…
        let from_events = event_store.backfill_trigger_id_from_events().await?;
        if from_events > 0 {
            log!(
                "[Engine] Backfilled {} thread_summaries rows: trigger_id NULL → first TriggerStarted event",
                from_events
            );
        }

        // …then translate any v5 hashes the pre-fix scheduler stamped into
        // `trigger_id` back to the raw `config.id` so the dropdown filter matches.
        // Subsequent runs update zero rows.
        let backfilled = event_store.backfill_trigger_id_v5_to_config_id().await?;
        if backfilled > 0 {
            log!(
                "[Engine] Backfilled {} thread_summaries rows: trigger_id v5-hash → config.id",
                backfilled
            );
        }

        // One-shot, idempotent: repos added before the `RepositoryAdded` event
        // existed have no name in `repositories` (once removed) or the event
        // log, so the filter showed their UUID. Recover the name from the path
        // basename preserved in `changes.repo_root`.
        let repo_names = event_store.backfill_repo_names_from_changes().await?;
        if repo_names > 0 {
            log!(
                "[Engine] Backfilled {} repo_names rows from changes.repo_root (pre-RepositoryAdded repos)",
                repo_names
            );
        }
        let python_runtime = PythonRuntime::new(workspace_path.clone())?;
        let app_manager = Arc::new(AppManager::new(&workspace_path)?);

        // The embedding model that powers vector memory is a multi-hundred-MB
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

        // Initialize scheduler schemas (notifications)
        // This ensures tables exist even before SchedulerManager is created
        crate::scheduler::NotificationStore::init_schema(&pool).await?;

        // Initialize credentials, preferences, and pinned apps schemas
        CredentialStore::init_schema(&pool).await?;
        PreferenceStore::init_schema(&pool).await?;
        PinnedAppStore::init_schema(&pool).await?;
        HeadlessBlocklist::init_schema(&pool).await?;
        BrowserLogins::init_schema(&pool).await?;

        // Resolve the OpenAI key once: a stored `openai` credential (Settings →
        // Providers) is preferred, then the OPENAI_API_KEY launch env var, then a
        // key auto-detected from the Codex CLI's auth file (apikey login) as the
        // lowest-precedence fallback. Used for the image provider and for routing
        // `gpt-*` background-task models through the MemoryExtractor.
        let openai_credential = match CredentialStore::get(&pool, "openai").await {
            Ok(Some(cred)) => Some((cred.auth_type, cred.auth_value)),
            Ok(None) => None,
            Err(e) => {
                log!("[Startup] Failed to read OpenAI credential: {}", e);
                None
            }
        };
        let openai_api_key = crate::llm::resolve_openai_api_key(
            openai_credential,
            std::env::var("OPENAI_API_KEY").ok(),
            crate::llm::openai::codex_detect::load(),
        )
        .map(|(key, _source)| key);

        let extractor = if vertex_project_id.is_empty() {
            log!("[Memory] No Vertex project configured — memory extraction disabled");
            None
        } else {
            let cache = vertex_token_cache
                .clone()
                .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(None)));
            Some(
                MemoryExtractor::with_location_handle(
                    vertex_project_id.clone(),
                    vertex_location.clone(),
                    cache,
                )?
                .with_openai_key(openai_api_key.clone()),
            )
        };

        // Memory index uses PostgreSQL + pgvector for vector search
        let memory_index = match PgVectorIndex::new(pool.clone()).await {
            Ok(index) => {
                let count = index.len().await.unwrap_or(0);
                log!("[Memory] Index initialized (pgvector, {} entries)", count);
                Some(index)
            }
            Err(e) => {
                log!(
                    "[Memory] Could not initialize index: {}. Memory search disabled.",
                    e
                );
                None
            }
        };

        // The startup re-embed sweep is NOT run here: the slot boots empty (the
        // model loads in the background), so every batch would just error
        // EMBEDDER_UNAVAILABLE. `spawn_embedder_load` runs the same sweep once it
        // installs the model.

        // Load user profile from artifacts, or use empty if doesn't exist
        let profile_path = workspace_path
            .join(crate::core::ARTIFACTS_DIR)
            .join("user_profile.md");
        let user_profile = std::fs::read_to_string(&profile_path).unwrap_or_default();
        if !user_profile.is_empty() {
            log!(
                "[Memory] Loaded user profile ({} chars)",
                user_profile.len()
            );
        }

        // Load user timezone from database, environment, or leave empty (LLM will ask)
        let user_timezone = match PreferenceStore::get(&pool, "timezone").await {
            Ok(Some(tz)) => tz,
            _ => String::new(),
        };

        if user_timezone.is_empty() {
            log!("[Engine] User timezone: not set (LLM will ask)");
        } else {
            log!("[Engine] User timezone: {}", user_timezone);
        }

        // Load user language preference from database
        let user_language = match PreferenceStore::get(&pool, "language").await {
            Ok(Some(lang)) => lang,
            _ => String::new(),
        };

        if user_language.is_empty() {
            log!("[Engine] User language: not set (will detect from conversation)");
        } else {
            log!("[Engine] User language: {}", user_language);
        }

        let repo_root = git_ops::main_worktree().await;

        // Resolve the engine-shipped `system-knowhow/` reference set. In a packaged
        // build the launcher stages it as the 7th resource and points
        // LUCIDOS_SYSTEM_KNOWHOW_DIR at it; dev/e2e leave the env var unset and fall
        // back to the source checkout's `<repo_root>/system-knowhow`. A set-but-missing
        // env var (mis-staged bundle) resolves to unavailable with a loud warning
        // rather than a bogus repo-root fallback. See
        // docs/plans/2026-07-07-package-system-knowhow-resource.md.
        let (system_knowhow_dir, system_knowhow_warning) = crate::core::resolve_system_knowhow_dir(
            std::env::var("LUCIDOS_SYSTEM_KNOWHOW_DIR").ok().as_deref(),
            &repo_root,
            crate::runtime::is_packaged(),
        );
        if let Some(warning) = &system_knowhow_warning {
            log!("{}", warning);
        }

        // Register the Lucidos repo so it appears in the Files view without manual
        // setup. Its id is derived from the repo's root-commit SHA (read from
        // disk), so a registry wipe / re-seed always recomputes the SAME id —
        // coding-agent threads bound to it never orphan.
        //
        // DEV-ONLY: gate on a genuine source checkout, via the shared
        // `has_lucidos_source()` predicate (the same signal behind the `/health`
        // `packaged` flag and the chat agent's coding-surface prompt — one
        // definition, so the three can't drift). On a packaged build it is false
        // and the `repo_root` above — from `main_worktree()` — falls back to the
        // workspace dir. Registering THAT under the reserved name "Lucidos" would
        // mis-label the user's workspace as the platform source (then hidden by
        // the compose picker's reserved-name handling). There is no Lucidos
        // source on packaged, so skip both the registration and the
        // source-thread backfill below.
        if crate::paths::has_lucidos_source() {
            let default_repo_root_commit = git_ops::root_commit_sha(&repo_root).await;
            // Engine-internal registration, so no actor. `register` announces
            // it only if the row was actually created or moved, which keeps a
            // plain restart from emitting `RepositoryAdded` (and re-firing
            // every trigger listening on it) every single boot.
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
                log!(
                    "[Startup] Failed to register default Lucidos repository: {}",
                    e
                );
            }

            // One-time, marker-guarded: re-point coding-agent threads orphaned by
            // the old random-UUID registry onto the default repo's deterministic
            // id (every lucidos/legacy thread targets the Lucidos source by
            // definition). Must run AFTER ensure_exists above so the live row
            // already carries the new id.
            let default_repo_det_id = crate::core::repositories::deterministic_id(
                default_repo_root_commit.as_deref(),
                &repo_root.to_string_lossy(),
            );
            match event_store
                .backfill_cc_repo_id_to_deterministic(default_repo_det_id)
                .await
            {
                Ok(n) if n > 0 => log!(
                    "[Engine] Backfilled {} thread_summaries rows: cc_repo_id → deterministic repo id {}",
                    n,
                    default_repo_det_id
                ),
                Ok(_) => {}
                Err(e) => log!("[Startup] cc_repo_id deterministic backfill failed: {}", e),
            }
        } else {
            log!("[Startup] Packaged build (no source checkout) — skipping Lucidos source repo registration");
        }

        // Auto-commit safe files (docs) if dirty — a dirty working tree blocks
        // change apply/revert which require a clean state.
        auto_commit_safe_files_if_dirty(&repo_root).await;

        // Orphaned Claude Code worktrees (in-flight sessions interrupted by crash)
        // are recovered after engine creation by calling recover_orphaned_worktrees().
        // Idle sessions stay idle — they're shown in the WAITING UI on reload.

        // Must run before list_pending()-based reconciliation below, so any
        // recovered pending row is visible to the stale-branch discard.
        match event_bus
            .changes_projection()
            .rebuild_missing_from_events()
            .await
        {
            Ok(0) => {}
            Ok(n) => log!("[Startup] Rebuilt {} missing change row(s) from events", n),
            Err(e) => log!("[Startup] rebuild_missing_from_events failed: {}", e),
        }

        // Clean up stale merge worktrees left by crashed apply_change operations.
        // The merge stays in the worktree; the change stays pending so the user can retry.
        // DB error on this read degrades to "skip cleanup this boot" — the next
        // restart retries, and the worktrees stay on disk until then.
        let stale_merges = match event_bus.changes_projection().with_merge_worktree().await {
            Ok(v) => v,
            Err(e) => {
                log!(
                    "[Startup] with_merge_worktree: {} — skipping stale merge cleanup",
                    e
                );
                Vec::new()
            }
        };
        for change in stale_merges {
            if let (Some(wt), Some(tb)) = (&change.merge_worktree_path, &change.merge_temp_branch) {
                let change_repo = std::path::PathBuf::from(&change.repo_root);
                // git_cmd cleanup is best-effort — the worktree/branch may already be gone.
                let _ = git_cmd(&["merge", "--abort"], std::path::Path::new(wt)).await;
                let _ = git_cmd(&["worktree", "remove", "--force", wt], &change_repo).await;
                let _ = git_cmd(&["branch", "-D", tb], &change_repo).await;
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
                log!(
                    "[Startup] Cleaned up stale merge worktree for change {}",
                    change.id
                );
            }
        }

        // Surface — but do NOT discard — pending changes whose branches no
        // longer exist. The user resolves these from Review explicitly; the
        // engine never auto-discards on the user's behalf. Apply on a
        // missing-branch row will fail with a useful error and the user
        // can then click Discard. DB error degrades to "skip surfacing this
        // boot"; the projection is still authoritative for Review's UI.
        let stale = match event_bus.changes_projection().list_pending().await {
            Ok(v) => v,
            Err(e) => {
                log!(
                    "[Startup] list_pending: {} — skipping stale branch surface log",
                    e
                );
                Vec::new()
            }
        };
        for change in stale {
            // Ask the change's OWN repo, not the Lucidos source worktree. An app
            // coding-agent change carries `repo_root = <workspace>` and an
            // external-repo one carries that repo, so probing `repo_root` here
            // asked the wrong git for a branch it was never going to have, and
            // logged the missing-branch alarm for every such change on every
            // boot. The stale_merges loop above already does this correctly.
            let change_repo = std::path::PathBuf::from(&change.repo_root);
            // `or_unknown(true)`: a probe that could not run is UNKNOWN, never a
            // "no". `git_cmd` returns `Err` for a spawn failure AND for its 30s
            // timeout, and boot competes with every other startup sweep for the
            // machine, so reading a timeout as "the branch is gone" would print a
            // false missing-branch alarm on exactly the boots that are busiest.
            let branch_ok = crate::engine::git_ops::git_answer(
                &["rev-parse", "--verify", &change.branch_name],
                &change_repo,
            )
            .await
            .or_unknown(true);
            if !branch_ok {
                log!(
                    "[Engine] Pending change {} references missing branch {} — left in Review for user to resolve",
                    change.id,
                    change.branch_name
                );
            }
        }

        let mcp_manager = crate::mcp::McpManager::new(pool.clone(), event_bus.clone());

        let user_dir = std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".lucidos"));

        if let Some(ref ud) = user_dir {
            crate::core::user_dir::ensure_git_init(ud);
        }

        // Wrap the active provider in the swappable handle BEFORE the struct is
        // assembled, so the credential subscriber below shares the very handle
        // stored on `Self.llm` and its writes are seen by every read site.
        let llm_handle: Arc<std::sync::RwLock<Arc<dyn LlmProvider>>> =
            Arc::new(std::sync::RwLock::new(llm));
        // Same treatment for the search chain: one handle, shared with the
        // subscriber, so a credential change reaches every read site.
        let web_search_handle: Arc<std::sync::RwLock<Arc<crate::llm::WebSearchChain>>> =
            Arc::new(std::sync::RwLock::new(web_search));

        // Inputs the runtime credential subscriber needs to rebuild a provider
        // identical to a fresh boot. Env-stable and mirror `main.rs`; the shared
        // handles (Vertex location/token cache, model registry) are cloned so the
        // rebuilt provider tracks live region/registry updates and reuses warm
        // Vertex tokens. Clone everything here — the originals are moved into the
        // other subscribers / `Self` below.
        let default_model = std::env::var("LUCIDOS_MODEL")
            .unwrap_or_else(|_| crate::core::DEFAULT_CHAT_MODEL.to_string());
        let provider_build_ctx = crate::llm::ProviderBuildContext {
            model_is_mock: default_model == "mock",
            default_model,
            vertex_project_id: vertex_project_id.clone(),
            vertex_location: vertex_location.clone(),
            vertex_token_cache: vertex_token_cache.clone(),
            model_registry: model_registry.clone(),
            boot_without_provider: crate::llm::boot_without_provider_enabled(),
        };

        // Kept out of the ctx move below: the engine holds its own handle to the
        // same shared map so the context trimmer can read each model's declared
        // context window.
        let engine_model_registry = model_registry.clone();

        spawn_vertex_region_subscriber(event_bus.subscribe(), vertex_location.clone());
        spawn_models_registry_subscriber(event_bus.subscribe(), model_registry, pool.clone());

        // Hot-swap the active LLM provider when a provider credential is added or
        // removed at runtime — the whole point of booting unconfigured. NOT
        // spawned under LUCIDOS_MODEL=mock, so the mock is never swapped to/from
        // (mock isolation).
        if !provider_build_ctx.model_is_mock {
            spawn_provider_credential_subscriber(
                event_bus.subscribe(),
                llm_handle.clone(),
                web_search_handle.clone(),
                pool.clone(),
                provider_build_ctx,
            );
        }

        // Build the wasmtime engine ONCE — both `proxy_modules` (compiled
        // here at startup) and `WasmSignerLayer::apply` (instantiated
        // per-request in `proxy.rs`) MUST share this same engine.
        // wasmtime interns `FuncType` identity per-engine, so a separate
        // engine for each side rejects the host imports as "incompatible
        // import type" / panics with "id from different slab".
        let wasm_engine = Arc::new(
            crate::api::proxy_wasm_signer::build_wasmtime_engine()
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?,
        );

        // Shared with SchedulerManager AND the Thread Queue (drain consults
        // trigger pause/deletion state), so all three see one registry.
        let trigger_configs: Arc<
            std::sync::RwLock<HashMap<String, crate::triggers::TriggerConfig>>,
        > = Arc::new(std::sync::RwLock::new(HashMap::new()));
        // Shared with the Thread Queue too: its overflow guard performs a real
        // trigger write, and a lock only the engine held would not serialize it.
        let trigger_write_lock = Arc::new(tokio::sync::Mutex::new(()));

        // Thread Queue admission control. Policy is event-sourced — the
        // latest CapacityPolicyChanged event is the configuration.
        let capacity_policy = thread_queue::ThreadQueue::load_policy(&pool).await;
        let thread_queue_manager = Arc::new(thread_queue::ThreadQueue::new(
            pool.clone(),
            event_bus.clone(),
            trigger_configs.clone(),
            workspace_path.clone(),
            trigger_write_lock.clone(),
            capacity_policy,
        ));

        Ok(Self {
            artifact_manager,
            event_store,
            python_runtime,
            browser_runtime,
            app_manager,
            // The swappable provider handle, shared with the credential
            // subscriber spawned above (built before this struct so both sides
            // hold the same lock).
            llm: llm_handle,
            web_search: web_search_handle,
            embedder,
            memory_index,
            extractor,
            vertex_project_id,
            vertex_location,
            vertex_token_cache,
            model_registry: engine_model_registry,
            openai_api_key,
            rebuilding_memory: AtomicBool::new(false),
            cancel_rebuild: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            backup_in_progress: AtomicBool::new(false),
            // `true`, not `false`: reaching here means the pool connected and the
            // migrator ran, so the database WAS answering a moment ago. Starting
            // false would make every engine report an outage until its first
            // probe lands. See `engine::db_health`.
            database_reachable: AtomicBool::new(true),
            build_state: std::sync::RwLock::new(crate::engine::engine_version::BuildState::Idle),
            update_check: std::sync::Mutex::new(Default::default()),
            source_behind_cache: std::sync::Mutex::new(Default::default()),
            disk_direction_cache: std::sync::Mutex::new(Default::default()),
            pending_commits_cache: std::sync::Mutex::new(Default::default()),
            self_heal_state: std::sync::Mutex::new(Default::default()),
            build_task: std::sync::Mutex::new(None),
            build_generation: std::sync::atomic::AtomicU64::new(0),
            served_frontend: std::sync::OnceLock::new(),
            served_frontend_source: std::sync::OnceLock::new(),
            frontend_refresh_generation: std::sync::atomic::AtomicU64::new(0),
            frontend_refresh_task: std::sync::Mutex::new(None),
            frontend_worktree_pin_warned: std::sync::atomic::AtomicBool::new(false),
            restart_actor: std::sync::Mutex::new(None),
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
                    match crate::api::proxy_wasm_signer::load_wasm_modules(&dir, &wasm_engine) {
                        Ok(m) => {
                            if !m.is_empty() {
                                log!(
                                    "[Startup] loaded {} WASM auth module(s) from {}",
                                    m.len(),
                                    dir.display()
                                );
                            }
                            m
                        }
                        Err(e) => {
                            // A bad .wasm shouldn't take the engine down — log
                            // loud, start with an empty map. Operators fix it
                            // and POST /proxy-modules/reload (Phase 9).
                            log!(
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
            user_profile: tokio::sync::RwLock::new(user_profile),
            user_timezone: tokio::sync::RwLock::new(user_timezone),
            user_language: tokio::sync::RwLock::new(user_language),
            event_bus: {
                // parent_callback_rx is stored temporarily in a thread-local;
                // it's extracted and wired up after Arc::new(engine) via start_parent_callback_listener().
                PARENT_CALLBACK_RX.with(|cell| cell.borrow_mut().replace(parent_callback_rx));
                event_bus
            },
            presence_tracker: crate::api::presence_pong::PresenceTracker::new(),
            sse_connections: crate::api::sse_connections::SseConnectionCounter::new(),
            pool,
            proxy_token_cache: Arc::new(crate::api::proxy_token_cache::ProxyTokenCache::new()),
            wasm_engine,
            pending_captures: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            pending_installs: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            pending_uninstalls: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            frontend_origin: std::sync::Mutex::new(None),
            agent_sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            loaded_knowhow: Arc::new(crate::engine::loaded_knowhow::LoadedKnowhowStore::new()),
            agent_runtimes: {
                let mut m: HashMap<CodingAgent, Arc<dyn AgentRuntime>> = HashMap::new();
                m.insert(CodingAgent::ClaudeCode, Arc::new(ClaudeCodeRuntime));
                m.insert(CodingAgent::Codex, Arc::new(CodexRuntime));
                m
            },
            last_cc_spawn: std::sync::Mutex::new(HashMap::new()),
            pending_app_spawn: std::sync::Mutex::new(HashMap::new()),
            cc_spawn_coalesce: agent_session::CcSpawnCoalescer::new(),
            cc_startup_semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
            workspace_repo_lock: Arc::new(tokio::sync::Mutex::new(())),
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
            apply_all_drive_tx: {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                APPLY_ALL_DRIVE_RX.with(|cell| cell.borrow_mut().replace(rx));
                tx
            },
            self_arc: std::sync::OnceLock::new(),
            trigger_configs,
            trigger_groups: Arc::new(std::sync::RwLock::new(HashMap::new())),
            trigger_group_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            trigger_write_lock,
            bash_background: crate::engine::tools::bash_background::BackgroundBashRegistry::new(),
            thread_queue: thread_queue_manager,
        })
    }

    /// Load CC commands cache from `.lucidos/cc-commands.json` (survives engine restarts).
    fn load_cc_commands_cache(workspace: &std::path::Path) -> HashMap<String, CcCommandsInfo> {
        let path = workspace.join(".lucidos/cc-commands.json");
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                log!("[ClaudeCode] Failed to parse CC commands cache: {}", e);
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        }
    }
}
