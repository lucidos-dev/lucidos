use lucidos_engine::api::{create_router, SharedEngine};
use lucidos_engine::engine::LucidosEngine;
use lucidos_engine::llm::{resolve_openai_api_key, OpenAiProvider, RoutingProvider, VertexProvider};
use lucidos_engine::log;
use lucidos_engine::scheduler::SchedulerManager;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;

async fn shutdown_signal(
    handle: axum_server::Handle,
    engine: SharedEngine,
    scheduler: Arc<tokio::sync::Mutex<SchedulerManager>>,
) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    // Legitimate stop signal. Use SIGUSR1 (not SIGTERM) so accidental `kill`
    // commands from Claude Code subprocess tests can't take the engine down. A test
    // that does `lsof -ti :5173 | xargs kill` will hit the engine's pid and
    // send the default SIGTERM — we install a SIGTERM ignorer below so the
    // engine survives. Legitimate stops (web-dev.sh kill_stale_processes,
    // stop.sh, /api/v1/restart's spawned web-dev.sh) all send SIGUSR1.
    #[cfg(unix)]
    let usr1 = async {
        signal::unix::signal(signal::unix::SignalKind::user_defined1())
            .expect("failed to install SIGUSR1 handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let usr1 = std::future::pending::<()>();

    // SIGTERM ignorer. Without an installed handler the kernel would deliver
    // SIGTERM to the default action (terminate the process), so a CC test
    // script that `xargs kill`s the engine's pid would still kill it. By
    // installing a handler that logs + loops, we catch the signal and refuse
    // to act on it. The handle is leaked deliberately — we want it to live
    // for the rest of the process lifetime.
    #[cfg(unix)]
    let _sigterm_ignorer = tokio::spawn(async {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM ignorer");
        loop {
            sigterm.recv().await;
            log!(
                "[Shutdown] Received SIGTERM — ignored. Use SIGUSR1 to stop \
                 (web-dev.sh kill_stale_processes / stop.sh / /api/v1/restart \
                 all do this) or send SIGKILL to force-terminate."
            );
        }
    });

    tokio::select! {
        _ = ctrl_c => {},
        _ = usr1 => {},
    }

    log!("\n[Shutdown] Shutting down gracefully...");

    // Stop the scheduler firing event-triggers before any cleanup event is
    // emitted. `shutdown_agent_sessions`/`shutdown_active_threads` below emit
    // terminator events (CodingAgentIdled, SessionEnded, ResponseAborted) that
    // otherwise fan out to triggers; a trigger script's `lucidos ...` callback
    // would then hit the HTTP API being torn down (line below) and die with a
    // spurious "<trigger> failed" push. The scheduler's own shutdown flag is
    // set much later (scheduler.shutdown()), too late to gate these events.
    engine.mark_shutting_down();

    // Gracefully stop Claude Code sessions — interrupts active work,
    // waits for CodingAgentIdled events (which persist cc_session_id),
    // then cancels remaining sessions. Must happen before HTTP shutdown
    // so the event bus can still persist events.
    engine.shutdown_agent_sessions().await;
    engine.shutdown_active_threads().await;

    // Signal the server to stop accepting new connections and drain existing ones
    handle.graceful_shutdown(Some(Duration::from_secs(10)));

    // Close browser to avoid orphaned Chrome processes
    engine.shutdown_browser().await;

    // Shutdown the scheduler
    if let Err(e) = scheduler.lock().await.shutdown().await {
        log!("[Shutdown] Error shutting down scheduler: {}", e);
    }

    log!("[Shutdown] Shutdown complete.");
}

async fn read_vertex_region_pref(database_url: &str) -> Option<String> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await
        .ok()?;
    sqlx::query_scalar::<_, String>(
        "SELECT value FROM preferences WHERE key = $1 AND device_id IS NULL",
    )
    .bind(lucidos_engine::core::PREF_VERTEX_REGION)
    .fetch_optional(&pool)
    .await
    .ok()
    .flatten()
    .filter(|s| !s.is_empty())
}

/// Build the direct-OpenAI provider from a resolved key, logging where the key
/// came from and degrading to `None` (rather than aborting startup) if the
/// reqwest client can't be built. Shared by both branches of
/// `load_direct_providers_and_registry` so the DB-up and DB-down paths honor
/// the OPENAI_API_KEY fallback identically.
fn build_openai_provider(
    credential: Option<(lucidos_engine::core::AuthType, String)>,
    env_key: Option<String>,
    default_model: &str,
) -> Option<OpenAiProvider> {
    match resolve_openai_api_key(credential, env_key) {
        Some((key, source)) => {
            log!("[Startup] OpenAI provider configured (key from {})", source);
            OpenAiProvider::new(key, default_model.to_string())
                .map_err(|e| log!("[Startup] Failed to build OpenAI provider: {}", e))
                .ok()
        }
        None => None,
    }
}

/// Open a short-lived pool to assemble the direct OpenAI + Anthropic providers
/// (from the `openai` / `anthropic` credentials set in Settings → Providers)
/// and the model→provider registry. Providers are built before
/// `LucidosEngine::new` creates its own pool, so — like `read_vertex_region_pref`
/// — this uses a throwaway connection. The OpenAI key prefers the stored
/// credential and falls back to `openai_env_key` (the `OPENAI_API_KEY` launch
/// env var). Returns `(openai, anthropic, id→provider map)`; each degrades to
/// `None`/empty on any DB or credential error so the engine still boots on its
/// other providers.
async fn load_direct_providers_and_registry(
    database_url: &str,
    default_model: &str,
    openai_env_key: Option<String>,
) -> (
    Option<OpenAiProvider>,
    Option<lucidos_engine::llm::AnthropicProvider>,
    std::collections::HashMap<String, lucidos_engine::llm::ProviderKind>,
) {
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            log!(
                "[Startup] Could not open pool for direct-provider credentials / model registry: {}",
                e
            );
            // No DB access, but the OPENAI_API_KEY env fallback must still work.
            let openai = build_openai_provider(None, openai_env_key, default_model);
            return (openai, None, std::collections::HashMap::new());
        }
    };

    let registry_map = lucidos_engine::llm::model_registry::load_from_db(&pool).await;

    let anthropic = match lucidos_engine::core::CredentialStore::get(&pool, "anthropic").await {
        Ok(Some(cred)) => {
            use lucidos_engine::core::AuthType;
            use lucidos_engine::llm::AnthropicAuth;
            let auth = match cred.auth_type {
                AuthType::ApiKey => Some(AnthropicAuth::ApiKey(cred.auth_value)),
                AuthType::Bearer => Some(AnthropicAuth::OAuthBearer(cred.auth_value)),
                other => {
                    log!(
                        "[Startup] Anthropic credential auth_type {} unsupported (expected api_key or bearer) — direct Anthropic disabled",
                        other
                    );
                    None
                }
            };
            match auth.and_then(|a| {
                lucidos_engine::llm::AnthropicProvider::new(a, default_model.to_string())
                    .map_err(|e| log!("[Startup] Failed to build Anthropic provider: {}", e))
                    .ok()
            }) {
                Some(p) => {
                    log!("[Startup] Direct Anthropic provider configured");
                    Some(p)
                }
                None => None,
            }
        }
        Ok(None) => None,
        Err(e) => {
            log!("[Startup] Failed to read Anthropic credential: {}", e);
            None
        }
    };

    // OpenAI: a stored `openai` credential wins; otherwise the env fallback.
    let openai_credential = match lucidos_engine::core::CredentialStore::get(&pool, "openai").await {
        Ok(Some(cred)) => Some((cred.auth_type, cred.auth_value)),
        Ok(None) => None,
        Err(e) => {
            log!("[Startup] Failed to read OpenAI credential: {}", e);
            None
        }
    };
    let openai = build_openai_provider(openai_credential, openai_env_key, default_model);

    (openai, anthropic, registry_map)
}

fn get_gcloud_project() -> Option<String> {
    Command::new("gcloud")
        .args(["config", "get-value", "project"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
}

fn raise_fd_limit() {
    use std::mem::MaybeUninit;
    unsafe {
        let mut rlim = MaybeUninit::<libc::rlimit>::uninit();
        if libc::getrlimit(libc::RLIMIT_NOFILE, rlim.as_mut_ptr()) == 0 {
            let mut rlim = rlim.assume_init();
            if rlim.rlim_cur < 8192 {
                rlim.rlim_cur = rlim.rlim_max.min(8192);
                libc::setrlimit(libc::RLIMIT_NOFILE, &rlim);
            }
        }
    }
}

/// Worker-thread stack size for the Tokio runtime.
///
/// The engine polls deeply-nested async chains on a single worker thread. A
/// trigger fire descends scheduler → Thread Queue executor → `process_trigger`
/// → the agentic loop → `execute_intent` → a *nested* intent agentic sub-loop →
/// tool execution, all as one un-spawned future. Tokio's default 2 MiB worker
/// stack is marginal for that depth: once the Thread Queue executor added a few
/// frames to the front of the chain, a trigger that ran an app-scoped intent
/// overflowed the worker stack and aborted the engine (SIGABRT), crash-looping
/// via boot-resume. The depth is bounded — `execute_intent` is excluded from
/// the intent sub-loop's tool set (`build_intent_tools`), so the sub-loop can't
/// nest further — so a larger fixed stack fully resolves it. 16 MiB is reserved
/// virtual address space (committed lazily per touched page), so the headroom is
/// effectively free.
const WORKER_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

/// Build the multi-threaded Tokio runtime the engine runs on. Mirrors what
/// `#[tokio::main]` produces (multi-thread, all drivers enabled, one worker per
/// CPU) but pins a larger worker-thread stack — see [`WORKER_THREAD_STACK_SIZE`].
/// Shared with the runtime test so it exercises the exact production config.
fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_THREAD_STACK_SIZE)
        .build()
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Handle --version / -V before any heavy startup (runtime, DB, TLS, env).
    // Output prepends the umbrella Lucidos release to the per-crate engine version.
    // Uses bare `println!` (not `log!`) because this is user-facing CLI output that
    // tooling parses — the timestamp/pid/label prefix from `log!` would break callers.
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        println!(
            "Lucidos {} (lucidos-engine {})",
            lucidos_engine::LUCIDOS_RELEASE,
            env!("CARGO_PKG_VERSION"),
        );
        return Ok(());
    }

    build_runtime()?.block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Multiple crates enable both aws-lc-rs and ring features on rustls,
    // so auto-detection fails — install a provider explicitly before any TLS use.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls CryptoProvider");

    // Load .env file if present (won't override existing env vars)
    let _ = dotenvy::dotenv();

    // Raise file descriptor limit — macOS defaults to 256 which is too low
    // for an engine running multiple Claude Code sessions, SSE streams, DB pool, and
    // a Vite dev proxy simultaneously.
    raise_fd_limit();

    log!("[Startup] Lucidos Engine starting...");

    // Log parent pid + process group + session id so a post-mortem can
    // verify the supervisor chain from the log alone ("did the bash
    // supervisor actually wrap this engine?"). The `log!` macro prepends
    // `[pid:N]` to every line already, so the engine's own pid is not
    // duplicated here.
    #[cfg(unix)]
    {
        // SAFETY: getppid / getpgrp / getsid are async-signal-safe and
        // take no pointer arguments — calling them in Rust is well-defined.
        let ppid = unsafe { libc::getppid() };
        let pgid = unsafe { libc::getpgrp() };
        let sid = unsafe { libc::getsid(0) };
        log!("[Startup] ppid={} pgid={} sid={}", ppid, pgid, sid);
    }

    // Use local workspace for development, /workspace for Docker
    let workspace_path = std::env::var("LUCIDOS_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            if PathBuf::from("/workspace").exists() {
                PathBuf::from("/workspace")
            } else {
                PathBuf::from("./workspace")
            }
        });

    std::fs::create_dir_all(&workspace_path)?;
    log!("[Startup] Using workspace: {}", workspace_path.display());

    // Load this workspace's own `.env` (override semantics) on top of the global
    // `.env` loaded above, so each workspace can carry its own environment — e.g.
    // a workspace-specific GitHub auth account via GH_CONFIG_DIR / GIT_SSH_COMMAND
    // that `gh` / `git push` from agent subprocesses pick up. Values land in the
    // engine's process env, so every spawned subprocess inherits them.
    match lucidos_engine::core::load_workspace_env(&workspace_path) {
        Some(path) => log!("[Startup] Loaded per-workspace .env from {}", path.display()),
        None => log!(
            "[Startup] No per-workspace .env under {} — skipping",
            workspace_path.join(lucidos_engine::core::DATA_DIR).display()
        ),
    }

    // Upgrade legacy `apis.json` (single `auth.type` per provider) to
    // the pipeline shape. Idempotent. Failure is fatal — better to
    // refuse to start than to silently lose proxy auth (operator can
    // restore from the backup the migration just wrote).
    match lucidos_engine::api::proxy_migration::migrate_apis_json_if_needed(&workspace_path) {
        Ok(lucidos_engine::api::proxy_migration::MigrationOutcome::Migrated { backup_path }) => {
            log!(
                "[Startup] migrated apis.json to pipeline shape (backup at {})",
                backup_path.display()
            );
        }
        Ok(_) => {}
        Err(e) => {
            log!("[Startup] apis.json migration failed: {}", e);
            return Err(format!("apis.json migration failed: {e}").into());
        }
    }

    // Get database URL from environment (set by Docker) or use default for local dev
    let database_url = lucidos_engine::core::database_url();
    log!("[Startup] Connecting to PostgreSQL...");

    // Vertex AI config — needed for Claude/Gemini models and memory extraction
    let project_id = std::env::var("VERTEX_PROJECT_ID")
        .ok()
        .or_else(get_gcloud_project)
        .unwrap_or_default();

    let vertex_region_env =
        std::env::var("VERTEX_REGION").unwrap_or_else(|_| "europe-west1".to_string());
    let initial_region = read_vertex_region_pref(&database_url)
        .await
        .unwrap_or(vertex_region_env);
    let vertex_region = lucidos_engine::llm::vertex::location_handle(initial_region);

    // Default model (used when no model_override is specified)
    let model = std::env::var("LUCIDOS_MODEL")
        .unwrap_or_else(|_| lucidos_engine::core::DEFAULT_CHAT_MODEL.to_string());

    log!("[Startup] Using default model: {}", model);

    // Shared model→provider routing map. Populated from the DB below for real
    // providers; left empty for mock. Cloned into RoutingProvider and handed to
    // the engine so the reload subscriber can hot-swap it on Model* events.
    let model_registry = lucidos_engine::llm::model_registry::empty();

    // Create LLM provider — mock mode bypasses real providers entirely
    let (llm, vertex_token_cache): (
        Arc<dyn lucidos_engine::llm::LlmProvider>,
        Option<lucidos_engine::llm::vertex::TokenCache>,
    ) = if model == "mock" {
        log!("[Startup] Mock LLM provider active — no external API calls");
        (
            Arc::new(lucidos_engine::llm::mock::MockProvider::new(model.clone())),
            None,
        )
    } else {
        let (vertex, vtc) = if !project_id.is_empty() {
            log!("[Startup] Vertex AI configured (project: {})", project_id);
            let cache: lucidos_engine::llm::vertex::TokenCache =
                std::sync::Arc::new(std::sync::Mutex::new(None));
            let provider = VertexProvider::with_location_handle(
                project_id.clone(),
                vertex_region.clone(),
                model.clone(),
                cache.clone(),
            )?;
            (Some(provider), Some(cache))
        } else {
            log!("[Startup] Vertex AI not configured — Claude/Gemini models unavailable");
            (None, None)
        };

        // Direct OpenAI + Anthropic providers (Settings → Providers) and the
        // DB-backed model→provider routing map, all read from a throwaway pool.
        // OpenAI prefers a stored `openai` credential and falls back to the
        // OPENAI_API_KEY launch env var.
        let openai_env_key = std::env::var("OPENAI_API_KEY").ok();
        let (openai, anthropic, registry_map) =
            load_direct_providers_and_registry(&database_url, &model, openai_env_key).await;
        if let Ok(mut guard) = model_registry.write() {
            *guard = registry_map;
        }

        if vertex.is_none() && openai.is_none() && anthropic.is_none() {
            panic!("No LLM provider configured. Set VERTEX_PROJECT_ID (Claude/Gemini via Vertex), configure an OpenAI or Anthropic credential (Settings → Providers) or OPENAI_API_KEY (GPT), or LUCIDOS_MODEL=mock (for testing).");
        }

        (
            Arc::new(RoutingProvider::new(
                vertex,
                openai,
                anthropic,
                model_registry.clone(),
                model,
            )),
            vtc,
        )
    };

    // Create engine with pgvector for embeddings
    let engine = LucidosEngine::new(
        workspace_path.clone(),
        &database_url,
        llm,
        project_id,
        vertex_region,
        vertex_token_cache,
        model_registry,
    )
    .await?;
    log!("[Startup] PostgreSQL connected");

    // Extract shared read-only resources before wrapping engine in Arc
    let event_store = engine.event_store().clone();
    let embedder = engine.embedder().clone();
    let memory_index = engine.memory_index().clone();

    let shared_engine: SharedEngine = Arc::new(engine);
    shared_engine.set_self_arc(&shared_engine);
    shared_engine.start_parent_callback_listener();
    shared_engine.start_apply_all_driver();

    // If the bash supervisor dropped a respawn sidecar (the previous engine
    // pid died unexpectedly), emit one EngineSupervisorRespawned event so
    // the respawn is recorded in the audit timeline. Emits before recovery
    // so the timeline ordering is "supervisor respawn → recovery → ...".
    // Best-effort: a missing sidecar (clean restart) is the common case.
    lucidos_engine::engine::supervisor_respawn_sidecar::emit_if_present(
        &workspace_path,
        &shared_engine.event_bus,
    )
    .await;

    // Auto-resolve permission cards orphaned by the previous engine's death.
    // Must run before the orphan running/waiting resets: emitting Resolved
    // flips status to 'running', which those resets then settle to 'idle'.
    lucidos_engine::engine::agent_recovery::recover_orphan_cc_permission_requests(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;
    // Same for the command-guard permission lane (ADR 0002): a chat turn parked
    // on a CommandPermissionRequested is dead after restart, so clear the card.
    lucidos_engine::engine::command_permission::recover_orphan_command_permission_requests(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;

    // Reset any threads stuck in 'running' from the previous engine process.
    // These are orphaned — no live task is processing them. The recovery below
    // will set the correct status (waiting for CC threads with changes, idle otherwise).
    if let Err(e) =
        sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE status = 'running'")
            .execute(shared_engine.pool())
            .await
    {
        log!("[Startup] Failed to reset orphaned running threads: {}", e);
    }

    // Reset CC threads stuck in 'waiting' with no pending proposal.
    // After restart, Claude Code sessions are dead — threads with no pending proposal
    // have nothing for the user to act on and should go idle. Chat threads can
    // no longer reach 'waiting' (ResponseAborted goes idle, ResponseFailed goes
    // to 'failed'), so the source='claude_code' scope is defensive.
    if let Err(e) = sqlx::query(
        "UPDATE thread_summaries SET status = 'idle', \
         coding_agent_proposed = FALSE, coding_agent_requires_restart = FALSE, \
         coding_agent_is_external_repo = FALSE, coding_agent_applying = FALSE \
         WHERE status = 'waiting' AND coding_agent_proposed = FALSE AND source = 'claude_code'",
    )
    .execute(shared_engine.pool())
    .await
    {
        log!("[Startup] Failed to reset orphaned waiting threads: {}", e);
    }

    // Reconcile active_children_count in either drift direction. Over-count: a
    // child Claude Code session canceled before emitting CodingAgentIdled leaves the
    // parent with a stale non-zero count. Under-count: recovery's synthetic
    // `CodingAgentIdled{reason=engine_restart_interrupt}` decrements as if the
    // child were terminal, but the child is only parked — the user's Continue
    // click re-increments via the `ContinueSignal` projection. If the user
    // restarts again before clicking Continue, the now-zero count stays stuck
    // without this sweep (the projection's +1 fires once per park/resume pair,
    // not as a safety net for drifted rows). The `> 0` guard the prior version
    // carried would skip exactly the rows that need repair.
    if let Err(e) = sqlx::query(
        "WITH running_child_counts AS ( \
             SELECT parent_thread_id, COUNT(*) AS cnt \
             FROM thread_summaries \
             WHERE parent_thread_id IS NOT NULL AND status = 'running' \
             GROUP BY parent_thread_id \
         ), \
         parents AS ( \
             SELECT DISTINCT parent_thread_id AS thread_id \
             FROM thread_summaries WHERE parent_thread_id IS NOT NULL \
         ) \
         UPDATE thread_summaries p \
         SET active_children_count = COALESCE(rc.cnt, 0)::int \
         FROM parents pa LEFT JOIN running_child_counts rc \
              ON rc.parent_thread_id = pa.thread_id \
         WHERE p.thread_id = pa.thread_id \
           AND p.active_children_count != COALESCE(rc.cnt, 0)::int",
    )
    .execute(shared_engine.pool())
    .await
    {
        log!("[Startup] Failed to reconcile active_children_count: {}", e);
    }

    // Reconcile total_children_count
    if let Err(e) = sqlx::query(
        "WITH child_counts AS ( \
           SELECT parent_thread_id, COUNT(*) AS cnt \
           FROM thread_summaries WHERE parent_thread_id IS NOT NULL \
           GROUP BY parent_thread_id \
         ) \
         UPDATE thread_summaries p SET total_children_count = cc.cnt \
         FROM child_counts cc \
         WHERE p.thread_id = cc.parent_thread_id \
           AND p.total_children_count != cc.cnt",
    )
    .execute(shared_engine.pool())
    .await
    {
        log!("[Startup] Failed to reconcile total_children_count: {}", e);
    }

    // Reconcile blocking_descendant_count. The orphan-running and
    // orphan-waiting resets above flip child rows from blocking states
    // (status='running'/'waiting'+coding_agent_proposed) to idle via direct
    // UPDATEs that bypass the projection's sampling wrapper — so the parent's
    // materialized count is not decremented. The subsequent recovery sweep
    // emits ResponseAborted / CodingAgentIdled through EventBus, but by then
    // `prev_sample` already shows status='idle' and the projection computes
    // delta=0, leaving the count stuck at the pre-restart value. Without this
    // reconciliation the parent's Archive button stays disabled forever (the
    // `descendants_block_archive` predicate in `resolve_actions` keys off
    // `blocking_descendant_count > 0`), and `archive_thread`'s cascade never
    // runs even though every descendant is genuinely idle.
    if let Err(e) =
        lucidos_engine::engine::event_bus::EventBus::rebuild_blocking_descendant_count(
            shared_engine.pool(),
        )
        .await
    {
        log!(
            "[Startup] Failed to reconcile blocking_descendant_count: {}",
            e
        );
    }

    // Start todo garbage collector BEFORE the recovery sweep below. Recovery
    // emits `ResponseAborted { cause: RecoveryAfterRestart }` for chat threads
    // that were mid-response when the engine died — those terminators are
    // broadcast on the bus and must reach the consumer so abandoned todos get
    // flipped. tokio broadcast channels do NOT replay history for late
    // subscribers, so a consumer spawned after recovery sees nothing.
    lucidos_engine::engine::todo_consumer::spawn(shared_engine.clone());

    // Recover orphaned Claude Code worktrees (in-flight sessions that were
    // interrupted by engine crash). Idle sessions stay idle — they're shown
    // in the WAITING UI for the user to resume/apply/discard.
    let recovering_threads = shared_engine.recover_orphaned_worktrees().await;
    shared_engine
        .recover_orphaned_threads(&recovering_threads)
        .await;

    // Recover orphan `ToolCalled` events (engine died mid-tool, no matching
    // `ToolResult` in the events table). Without this, the next LLM call on
    // the affected thread reconstructs an assistant `tool_use` block whose
    // pair is missing — Anthropic 400s with "tool_use ids were found without
    // tool_result blocks immediately after". Mirror of the inner-tool layer
    // for the ResponseAborted recovery above.
    shared_engine.recover_orphan_tool_calls().await;

    // Reconcile `thread_summaries.coding_agent_has_diff` with on-disk git state for
    // every active CC thread. Live updates flow through ChangeProposed /
    // ChangeApplied / ChangeDiscarded / ThreadArchived projection handlers,
    // driven by the aggregate end-of-turn emit. Mid-turn commits don't update
    // the projection (the per-commit hook is gone), so this startup sweep is
    // the authoritative reconciliation against on-disk git reality before the
    // HTTP server starts serving frontend SSE — the WaitingBanner Diff button's
    // signal must reflect git from the first connection.
    lucidos_engine::engine::agent_recovery::refresh_coding_agent_has_diff_on_startup(
        shared_engine.pool(),
        shared_engine.workspace_path(),
    )
    .await;

    // Propose changes that were never surfaced at idle: any idle coding-agent
    // thread that has a committed branch diff but no pending change. This
    // recovers threads wedged by the now-removed bg-bash propose-gate (whose
    // only escape used to be a 5-minute nudge or a manual seed-change POST),
    // and is a general safety net for any missed idle-proposal. Steady-state
    // it's a no-op — `should_propose_change_at_idle` already fires for every
    // clean idle — so it only does work on the restart that lands this change
    // (and any future anomaly). See the function docstring for the per-thread
    // eligibility checks.
    lucidos_engine::engine::agent_recovery::propose_held_back_changes_on_startup(
        shared_engine.pool(),
        &shared_engine.event_bus,
        shared_engine.workspace_path(),
    )
    .await;

    // Start memory indexer — subscribes to EventBus and indexes chat events
    lucidos_engine::engine::memory_consumer::spawn(shared_engine.clone());

    // Start the worktree cleanup worker. See `engine::worktree_cleanup` module docs.
    lucidos_engine::engine::worktree_cleanup::WorktreeCleanup::spawn(
        shared_engine.pool().clone(),
        std::sync::Arc::new(shared_engine.event_bus.clone()),
        workspace_path.clone(),
        shared_engine.worktree_cleanup_active_threads(),
    );

    // Start the CC spawn dispatcher (Phase 5, Task 5.2).
    //
    // Subscribes to the EventBus and dispatches CC spawns based on trigger
    // events. ContinuationRequested triggers are production-active — they push a
    // SpawnRequest::Continue onto the dispatcher's outbound channel, which
    // a receiver task on LucidosEngine consumes (start_spawn_request_consumer
    // below). MessageReceived triggers stay in SHADOW mode: the chat HTTP
    // handler still owns spawning for those (its post-processing — auto-apply,
    // ChangesUpdated SSE, orphan re-submission, ResponseFailed — is not yet
    // event-driven). See `spawn_dispatcher` module docs and
    // `docs/plans/2026-04-24-cc-resume-spike-q7.md`.
    let (_dispatcher_handle, spawn_request_rx) =
        lucidos_engine::engine::spawn_dispatcher::SpawnDispatcher::spawn(
            shared_engine.pool().clone(),
            std::sync::Arc::new(shared_engine.event_bus.clone()),
        );
    shared_engine.start_spawn_request_consumer(spawn_request_rx);

    // Start the external watchdog. Scans agent_sessions every 30 s from
    // outside any per-thread `select!` — catches the May-2026 "stuck for
    // 68 min" failure mode where the in-loop watchdog was starved by a
    // wedged event handler. Emits ContinuationRequested (NOT ResponseAborted) so
    // the dispatcher above auto-resumes without any user-visible "Aborted"
    // terminal. See `engine::agent_session::external_watchdog`.
    let _watchdog_handle = shared_engine.spawn_external_watchdog();

    // Rebuild the Thread Queue's in-memory state from the `thread_queue`
    // projection BEFORE the scheduler starts (the scheduler is the first
    // submission path to come alive — loading the persisted backlog first
    // keeps per-trigger FIFO intact across the restart). Admitted rows whose
    // work died with the previous process re-queue; spawn rows whose thread
    // already materialized hand off to the thread-level recovery above.
    shared_engine.thread_queue.recover_persisted_entries().await;

    // Keep the in-memory user-initiated pool a faithful mirror of each thread's
    // `thread_summaries.status`: one subscriber reconciles the slot on every
    // status change (running → occupies one slot; parked / idle / terminal →
    // none; resume / continuation / restart → re-added). Without this the slot
    // is freed only when the chat task returns — which never happens while a
    // coding-agent thread is parked on an AskUserQuestion / permission prompt —
    // and, before reconcile, an answered thread that resumed running showed as
    // "Nothing running". Subscribe before the API server (the chat handler that
    // acquires user slots) comes alive.
    shared_engine.thread_queue.spawn_settle_subscriber();

    // Use the engine's shared pool for scheduler
    let pool = shared_engine.pool().clone();

    // Start scheduler before HTTP server
    let scheduler_engine = shared_engine.clone();
    let mut scheduler = SchedulerManager::new(scheduler_engine, pool.clone()).await?;
    scheduler.start().await?;
    let scheduler = Arc::new(tokio::sync::Mutex::new(scheduler));

    // Start draining the Thread Queue only now — drain consults trigger
    // pause/deletion state, which the scheduler's event replay just loaded.
    // Also runs the periodic safety-net drain + backlog-age notifications.
    shared_engine.thread_queue.start_draining();

    // Start API server with graceful shutdown
    let started_at = chrono::Utc::now();
    let app = create_router(
        shared_engine.clone(),
        pool,
        event_store,
        embedder,
        memory_index,
        workspace_path,
        scheduler.clone(),
        started_at,
    );
    let api_port = std::env::var("LUCIDOS_API_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3000);
    // Bind to [::] (dual-stack) — accepts both IPv4 and IPv6 connections.
    // macOS defaults to IPV6_V6ONLY=0, so [::]:port handles IPv4 too.
    // This avoids ECONNREFUSED when clients resolve localhost to ::1.
    let addr = SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, api_port));

    let handle = axum_server::Handle::new();
    tokio::spawn(shutdown_signal(
        handle.clone(),
        shared_engine.clone(),
        scheduler,
    ));

    // Detect TLS certs — if present, serve HTTPS with HTTP/2
    let tls_cert = std::env::var("LUCIDOS_TLS_CERT").ok();
    let tls_key = std::env::var("LUCIDOS_TLS_KEY").ok();

    if let (Some(cert_path), Some(key_path)) = (tls_cert, tls_key) {
        let tls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path).await?;
        log!(
            "[Startup] API server listening on https://[::]:{}  (HTTP/2 + TLS, dual-stack)",
            api_port
        );
        axum_server::bind_rustls(addr, tls_config)
            .handle(handle)
            .serve(app.into_make_service())
            .await?;
    } else {
        log!(
            "[Startup] API server listening on http://[::]:{}  (dual-stack)",
            api_port
        );
        axum_server::bind(addr)
            .handle(handle)
            .serve(app.into_make_service())
            .await?;
    }

    Ok(())
}

#[cfg(test)]
mod runtime_tests {
    use super::{build_runtime, WORKER_THREAD_STACK_SIZE};

    /// Recurse with a large live stack frame per level to emulate the engine's
    /// deep, un-spawned async poll chain (a trigger fire descending the agentic
    /// loop into a nested `execute_intent` sub-loop and a tool). Combining
    /// `#[inline(never)]`, `black_box`, and reading `buf` *after* the recursive
    /// call defeats tail-call and dead-store optimization so every level
    /// genuinely consumes stack.
    #[inline(never)]
    fn consume_stack(depth: usize) -> u64 {
        let mut buf = [0u8; 64 * 1024]; // 64 KiB of stack per frame
        let idx = depth % buf.len();
        buf[idx] = depth as u8;
        std::hint::black_box(&buf);
        let deeper = if depth == 0 { 0 } else { consume_stack(depth - 1) };
        deeper.wrapping_add(buf[idx] as u64)
    }

    /// Regression for the trigger stack-overflow (SIGABRT) crash: a cron/event
    /// trigger that ran an app-scoped intent descended
    /// `executor → process_trigger → agentic loop → execute_intent → intent
    /// sub-loop → tool` as one future and overflowed tokio's default 2 MiB
    /// worker stack. The engine's runtime must give that bounded-but-deep chain
    /// real headroom.
    #[test]
    fn worker_stack_holds_deep_nested_async_chain() {
        // Must comfortably exceed tokio's 2 MiB default that overflowed in prod.
        // Checked at compile time — the invariant is over a const, so a runtime
        // assert would be dead weight.
        const {
            assert!(
                WORKER_THREAD_STACK_SIZE >= 8 * 1024 * 1024,
                "worker stack must exceed tokio's 2 MiB default with headroom"
            );
        }

        // Run a chain needing ~4 MiB of stack (65 levels * 64 KiB) on a worker
        // thread of the *production* runtime builder. On the old 2 MiB default
        // this aborts the process (the original SIGABRT); on the configured
        // stack it completes cleanly.
        let rt = build_runtime().expect("runtime builds");
        let sum = rt.block_on(async {
            tokio::spawn(async { consume_stack(64) })
                .await
                .expect("deep-stack task joins without overflowing the worker stack")
        });
        let expected: u64 = (0..=64).sum();
        assert_eq!(sum, expected, "deep recursion ran every level to completion");
    }
}
