use lucidos_engine::api::{create_router, SharedEngine};
use lucidos_engine::engine::LucidosEngine;
use lucidos_engine::llm::{OpenAiProvider, RoutingProvider, VertexProvider};
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

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    log!("\n[Shutdown] Shutting down gracefully...");

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

    // Update user profile before shutdown
    engine.update_user_profile().await;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Handle --version / -V before any heavy startup (DB, TLS, env loading).
    // Output prepends the umbrella Lucidos release to the per-crate engine version.
    if std::env::args().skip(1).any(|a| a == "--version" || a == "-V") {
        println!(
            "Lucidos {} (lucidos-engine {})",
            lucidos_engine::LUCIDOS_RELEASE,
            env!("CARGO_PKG_VERSION"),
        );
        return Ok(());
    }

    // Multiple crates enable both aws-lc-rs and ring features on rustls,
    // so auto-detection fails — install a provider explicitly before any TLS use.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls CryptoProvider");

    // Load .env file if present (won't override existing env vars)
    let _ = dotenvy::dotenv();

    // Raise file descriptor limit — macOS defaults to 256 which is too low
    // for an engine running multiple CC sessions, SSE streams, DB pool, and
    // a Vite dev proxy simultaneously.
    raise_fd_limit();

    log!("[Startup] Lucidos Engine starting...");

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
            );
            (Some(provider), Some(cache))
        } else {
            log!("[Startup] Vertex AI not configured — Claude/Gemini models unavailable");
            (None, None)
        };

        let openai = match std::env::var("OPENAI_API_KEY") {
            Ok(api_key) => {
                log!("[Startup] OpenAI API configured");
                Some(OpenAiProvider::new(api_key, model.clone()))
            }
            Err(_) => None,
        };

        if vertex.is_none() && openai.is_none() {
            panic!("No LLM provider configured. Set VERTEX_PROJECT_ID (for Claude/Gemini), OPENAI_API_KEY (for GPT), or LUCIDOS_MODEL=mock (for testing).");
        }

        (Arc::new(RoutingProvider::new(vertex, openai, model)), vtc)
    };

    // Create engine with pgvector for embeddings
    let engine = LucidosEngine::new(
        workspace_path.clone(),
        &database_url,
        llm,
        project_id,
        vertex_region,
        vertex_token_cache,
    )
    .await?;
    log!("[Startup] PostgreSQL connected");

    // Generate user profile if it doesn't exist (uses same code path as session-end updates)
    if !engine.has_user_profile().await {
        log!("[Startup] Generating initial user profile...");
        engine.update_user_profile().await;
    }

    // Extract shared read-only resources before wrapping engine in Arc
    let event_store = engine.event_store().clone();
    let embedder = engine.embedder().clone();
    let memory_index = engine.memory_index().clone();

    let shared_engine: SharedEngine = Arc::new(engine);
    shared_engine.set_self_arc(&shared_engine);
    shared_engine.start_parent_callback_listener();

    // Auto-resolve permission cards orphaned by the previous engine's death.
    // Must run before the orphan running/waiting resets: emitting Resolved
    // flips status to 'running', which those resets then settle to 'idle'.
    lucidos_engine::engine::agent_recovery::recover_orphan_cc_permission_requests(
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

    // Reset CC threads stuck in 'waiting' with no pending changes.
    // After restart, CC sessions are dead — threads with cc_has_changes=false
    // have nothing for the user to act on and should go idle. Chat threads can
    // no longer reach 'waiting' (ResponseAborted goes idle, ResponseFailed goes
    // to 'failed'), so the source='claude_code' scope is defensive.
    if let Err(e) = sqlx::query(
        "UPDATE thread_summaries SET status = 'idle', \
         cc_has_changes = FALSE, cc_requires_restart = FALSE, \
         cc_is_external_repo = FALSE, cc_applying = FALSE \
         WHERE status = 'waiting' AND cc_has_changes = FALSE AND source = 'claude_code'",
    )
    .execute(shared_engine.pool())
    .await
    {
        log!("[Startup] Failed to reset orphaned waiting threads: {}", e);
    }

    // Reconcile active_children_count — if all children are idle/waiting but
    // the parent still has a non-zero count (e.g., a child CC session was
    // canceled before emitting CodingAgentIdled), reset the count to match reality.
    if let Err(e) = sqlx::query(
        "UPDATE thread_summaries p SET active_children_count = \
         COALESCE((SELECT COUNT(*) FROM thread_summaries c \
           WHERE c.parent_thread_id = p.thread_id AND c.status = 'running'), 0) \
         WHERE p.active_children_count > 0",
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
    // events. ContinueSignal triggers are production-active — they push a
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

    // Use the engine's shared pool for scheduler
    let pool = shared_engine.pool().clone();

    // Start scheduler before HTTP server
    let scheduler_engine = shared_engine.clone();
    let mut scheduler = SchedulerManager::new(scheduler_engine, pool.clone()).await?;
    scheduler.start().await?;
    let scheduler = Arc::new(tokio::sync::Mutex::new(scheduler));

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
