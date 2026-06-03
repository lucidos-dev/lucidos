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
            agent_session::external_watchdog::EXTERNAL_WATCHDOG_LIMIT_MS,
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
                                            },
                                            meta: crate::engine::thread_events::EventMeta {
                                                channel: Some(
                                                    crate::engine::thread_events::EventChannel::ClaudeCode,
                                                ),
                                                actor: continue_actor,
                                                ..crate::engine::thread_events::EventMeta::NONE
                                            },
                                        },
                                        "[SpawnConsumer] ContinuationStarted (continue)",
                                    )
                                    .await;
                            }

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
                            let result = engine
                                .run_direct_agent(
                                    request_id,
                                    thread_id,
                                    crate::engine::agent_recovery::CONTINUE_RESUME_USER_MESSAGE,
                                    None,
                                    event_id,
                                    None,
                                    &cancel_token,
                                    None,
                                    None,
                                    None,
                                    None,
                                    resume_sid,
                                    None,
                                    None,
                                    None,
                                )
                                .await;
                            if let Err(e) = result {
                                crate::log!(
                                    "[SpawnConsumer] Continue thread={} event={} failed: {}",
                                    thread_id,
                                    event_id,
                                    e
                                );
                            }
                        }
                    }
                });
            }
            crate::log!("[SpawnConsumer] channel closed — consumer exiting");
        });
    }

    pub async fn new(
        workspace_path: PathBuf,
        database_url: &str,
        llm: Arc<dyn LlmProvider>,
        vertex_project_id: String,
        vertex_location: crate::llm::vertex::LocationHandle,
        vertex_token_cache: Option<crate::llm::vertex::TokenCache>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Install the subprocess-origin token before anything spawns a
        // subprocess. See `api::actor` module docs.
        crate::api::actor::init_agent_origin_token(Uuid::new_v4().to_string());

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
        if workspace_was_uninitialized {
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
        sqlx::migrate!().run(&pool).await?;

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
        let python_runtime = PythonRuntime::new(workspace_path.clone())?;
        let app_manager = Arc::new(AppManager::new(&workspace_path)?);

        let embedder = Arc::new(FastEmbedProvider::new()?);

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

        let openai_api_key = std::env::var("OPENAI_API_KEY").ok();

        let extractor = if vertex_project_id.is_empty() {
            log!("[Memory] No Vertex project configured — memory extraction disabled");
            None
        } else {
            let cache = vertex_token_cache
                .clone()
                .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(None)));
            Some(MemoryExtractor::with_location_handle(
                vertex_project_id.clone(),
                vertex_location.clone(),
                cache,
            )?)
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

        // Gated on memory_index because it owns the schema; without it
        // memory_entries may not exist.
        if let Some(index) = memory_index.clone() {
            let embedder = embedder.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::memory::reembed::reembed_stale(&index, embedder).await {
                    log!(@Memory, "Re-embed task failed: {}", e);
                }
            });
        }

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

        let system_knowhow_dir = {
            let candidate = repo_root.join("system-knowhow");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        };

        // Register the Lucidos repo so it appears in the Files view without manual setup
        if let Err(e) = crate::core::repositories::RepositoryStore::ensure_exists(
            &pool,
            Self::DEFAULT_REPO_NAME,
            &repo_root.to_string_lossy(),
        )
        .await
        {
            log!(
                "[Startup] Failed to register default Lucidos repository: {}",
                e
            );
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
            let branch_ok = git_cmd(&["rev-parse", "--verify", &change.branch_name], &repo_root)
                .await
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !branch_ok {
                log!(
                    "[Engine] Pending change {} references missing branch {} — left in Review for user to resolve",
                    change.id,
                    change.branch_name
                );
            }
        }

        let mcp_manager = crate::mcp::McpManager::new(pool.clone());

        let user_dir = std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".lucidos"));

        if let Some(ref ud) = user_dir {
            crate::core::user_dir::ensure_git_init(ud);
        }

        spawn_vertex_region_subscriber(event_bus.subscribe(), vertex_location.clone());

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

        Ok(Self {
            artifact_manager,
            event_store,
            python_runtime,
            browser_runtime,
            app_manager,
            llm,
            embedder,
            memory_index,
            extractor,
            vertex_project_id,
            vertex_location,
            vertex_token_cache,
            openai_api_key,
            rebuilding_memory: AtomicBool::new(false),
            cancel_rebuild: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            backup_in_progress: AtomicBool::new(false),
            restore_state: Arc::new(std::sync::RwLock::new(
                crate::core::backup::RestoreState::default(),
            )),
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
                m
            },
            last_cc_spawn: std::sync::Mutex::new(HashMap::new()),
            pending_app_spawn: std::sync::Mutex::new(HashMap::new()),
            cc_spawn_coalesce: agent_session::CcSpawnCoalescer::new(),
            cc_startup_semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
            workspace_repo_lock: Arc::new(tokio::sync::Mutex::new(())),
            mcp_manager,
            pending_mcp_consent: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            pending_cc_permission: Arc::new(std::sync::Mutex::new(
                cc_permission::CcPermissionState::default(),
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
            trigger_configs: Arc::new(std::sync::RwLock::new(HashMap::new())),
            trigger_groups: Arc::new(std::sync::RwLock::new(HashMap::new())),
            trigger_group_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            bash_background: crate::engine::tools::bash_background::BackgroundBashRegistry::new(),
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
