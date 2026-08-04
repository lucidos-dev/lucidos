use lucidos_engine::api::{create_router, SharedEngine};
use lucidos_engine::engine::LucidosEngine;
use lucidos_engine::llm::{build_active_provider, ProviderBuildContext, ProviderBuildOutcome};
use lucidos_engine::log;
use lucidos_engine::net_config;
use lucidos_engine::scheduler::SchedulerManager;
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

    // Emit the boundary "Switched to new version" / abort events NOW, at real
    // teardown — never at switch-request time, so nothing shows "Switched/Aborted"
    // while the old engine is still alive through a dev rebuild. Reads the actor
    // the switch handler stashed: a device actor → "You" attribution + recovery
    // auto-resumes in-flight coding-agent threads; `None` (a bare stop.sh /
    // external SIGUSR1, or a crash — which never reaches this path) → System
    // attribution + manual Continue. Runs BEFORE the session sweeps below so its
    // `external_terminal_emitted` flags suppress their duplicate emits.
    engine
        .abort_in_flight_for_restart(engine.take_restart_actor())
        .await;

    // Gracefully stop coding-agent sessions — interrupts active work,
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

/// One-time `git --version` boot preflight.
///
/// `git` is a hard dependency of the coding-agent / Apply / worktree flow
/// (`git_ops` drives ~89 bare `git` call sites) but NOT of chat. So a missing or
/// broken `git` must surface as a loud, actionable warning at boot — NOT a fatal
/// exit: aborting would brick a chat-only packaged install on a Mac with no
/// Xcode Command Line Tools and respawn-loop it under launchd (the boot-splash
/// hang this audit explicitly avoids). Without this the failure only appears deep
/// in an apply/commit as a cryptic `git … failed`. On the launchd minimal PATH
/// `/usr/bin/git` is the Command-Line-Tools shim, which ENOENTs when CLT isn't
/// installed.
fn git_preflight() {
    match std::process::Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            log!(
                "[Startup] git: {}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
        Ok(out) => log!(
            "[Startup] WARNING: `git --version` exited {} — coding agents, Apply, and repo \
             operations will fail. On macOS install the Command Line Tools: `xcode-select --install`",
            out.status
        ),
        Err(e) => log!(
            "[Startup] WARNING: `git` not found ({e}) — coding agents, Apply, and repo \
             operations will fail. On macOS install the Command Line Tools: `xcode-select --install`"
        ),
    }
}

/// One-time `python3 --version` boot preflight — `git_preflight`'s sibling.
///
/// `python3` backs the `run_python` tool (venv creation spawns a bare
/// `python3`, see `runtime/python.rs`) but nothing else, so — like git — a
/// missing interpreter must be a loud, actionable boot warning, never a fatal
/// exit. On the launchd minimal PATH `/usr/bin/python3` is the
/// Command-Line-Tools shim, which errors when CLT isn't installed; without
/// this preflight that only surfaced as a cryptic venv-creation failure on the
/// first `run_python` call.
fn python_preflight() {
    match std::process::Command::new("python3")
        .arg("--version")
        .output()
    {
        Ok(out) if out.status.success() => {
            log!(
                "[Startup] python3: {}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
        Ok(out) => log!(
            "[Startup] WARNING: `python3 --version` exited {} — the run_python tool will fail. \
             On macOS install the Command Line Tools: `xcode-select --install`",
            out.status
        ),
        Err(e) => log!(
            "[Startup] WARNING: `python3` not found ({e}) — the run_python tool will fail. \
             On macOS install the Command Line Tools: `xcode-select --install`"
        ),
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

    // Print the baked ENGINE_BUILD_ID and exit. A running engine forks the on-disk
    // binary with this flag to compare build ids (the dev "new version available"
    // check — mirrors `lucidos-gateway --build-id`). Bare `println!`: machine-read
    // stdout, so no `log!` prefix.
    if std::env::args().skip(1).any(|a| a == "--build-id") {
        println!("{}", lucidos_engine::ENGINE_BUILD_ID);
        return Ok(());
    }

    // The workspace gateway is now the standalone `lucidos-gateway` binary
    // (ADR 0014 §1) — it spawns this engine per workspace via LUCIDOS_ENGINE_BIN.
    // A stray `--gateway` flag is a misconfiguration; fail loudly rather than
    // silently booting a single engine on the gateway's port.
    if std::env::args().skip(1).any(|a| a == "--gateway") {
        return Err("`--gateway` was removed from lucidos-engine; run the \
                    `lucidos-gateway` binary instead (ADR 0014)"
            .into());
    }

    // One-shot restore subcommand: the workspace gateway shells out to this to
    // decrypt + unpack + pg_restore a LOCAL backup archive INTO a workspace dir +
    // database it already provisioned (the gateway can't link the engine crate —
    // ADR 0014 §1 — so it reaches the restore logic via this binary). The engine
    // server the gateway then spawns runs migrations at construction, upgrading an
    // older-schema restore. Handled before the heavy server boot below.
    if std::env::args().nth(1).as_deref() == Some("restore-archive") {
        return build_runtime()?.block_on(run_restore_archive());
    }

    build_runtime()?.block_on(run())
}

/// `lucidos-engine restore-archive --file <enc> --workspace-dir <dir>`.
///
/// Restores a local encrypted backup archive into the given (already-provisioned)
/// workspace directory + database. The base64 key comes from `LUCIDOS_RESTORE_KEY`
/// and the connection string from `DATABASE_URL` — both via env so neither the
/// key nor a DB password ever lands in argv (argv is visible in `ps`). Prints
/// `LUCIDOS_RESTORE_PHASE=<phase>:<pct>` lines to stdout so the gateway can
/// surface coarse progress in the picker; exits non-zero with the error on stderr.
async fn run_restore_archive() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::io::Write;

    let mut file: Option<PathBuf> = None;
    let mut workspace_dir: Option<PathBuf> = None;
    let mut it = std::env::args().skip(2); // skip bin name + "restore-archive"
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--file" => file = it.next().map(PathBuf::from),
            "--workspace-dir" => workspace_dir = it.next().map(PathBuf::from),
            other => {
                return Err(format!("restore-archive: unexpected argument '{other}'").into());
            }
        }
    }
    let file = file.ok_or("restore-archive: --file is required")?;
    let workspace_dir = workspace_dir.ok_or("restore-archive: --workspace-dir is required")?;

    let _ = dotenvy::dotenv();
    let key_b64 = std::env::var("LUCIDOS_RESTORE_KEY")
        .map_err(|_| "restore-archive: LUCIDOS_RESTORE_KEY env var is required")?;
    let key = lucidos_engine::core::backup::crypto::key_from_base64(&key_b64)
        .map_err(|e| format!("restore-archive: invalid key: {e}"))?;
    let database_url = lucidos_engine::core::database_url();

    lucidos_engine::core::backup::restore_archive_into(
        &workspace_dir,
        &database_url,
        &key,
        &file,
        |phase, cur, _total| {
            // One progress line per tick; the gateway parses the latest phase for
            // the picker's restore-status. Best-effort — a write failure is fine.
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "LUCIDOS_RESTORE_PHASE={phase}:{cur}");
            let _ = out.flush();
        },
    )
    .await?;
    Ok(())
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
    // for an engine running multiple coding-agent sessions, SSE streams, the DB
    // pool, and static file serving simultaneously.
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

    // Prepend the common user-install bin dirs (Homebrew, ~/.local/bin, npm)
    // to the process PATH before anything spawns or probes tools. A packaged
    // engine (launchd LaunchAgent / systemd --user / the .app) inherits the
    // service manager's minimal PATH, which would ENOENT bare-name tools —
    // `claude`/`codex` fallbacks, chat bash/python shell-outs, MCP servers —
    // that resolve fine in a dev shell. No-op on a dev PATH (dedupe).
    lucidos_engine::core::user_path::augment_process_path();

    git_preflight();
    python_preflight();

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

    // The legacy per-workspace `data/.env` is retired in favour of the DB-backed
    // environment_variables store. Its contents are migrated into the DB and the
    // file removed below, AFTER the engine (and thus the DB + migrations) is up —
    // see the `migrate_env_file_to_db` call after `LucidosEngine::new`.

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

    // Vertex AI config — needed for Claude/Gemini models and memory extraction.
    // Resolve the project WITHOUT requiring the `gcloud` binary so a packaged
    // build works from the user's existing ADC: env → ADC `quota_project_id` /
    // gcloud config `[core] project` (file reads) → `gcloud config` subprocess
    // (dev fallback).
    let project_id = std::env::var("VERTEX_PROJECT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(lucidos_engine::llm::vertex::adc::project_from_files)
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

    // Decide + construct the LLM provider via the shared library builder
    // (`llm::build_active_provider`), so this boot path and the runtime
    // credential subscriber (`spawn_provider_credential_subscriber`) produce an
    // identical provider. `LUCIDOS_MODEL=mock` is the explicit E2E opt-in
    // (deterministic MockProvider); otherwise a real provider (Vertex / OpenAI /
    // Anthropic / OpenRouter / local) is preferred; otherwise
    // `LUCIDOS_BOOT_WITHOUT_PROVIDER` decides between booting an
    // UnconfiguredProvider (a packaged build's first run — boots into onboarding,
    // returns a clear "configure a provider" error on chat) and the dev/docker
    // fail-fast panic. The decision matrix lives in `llm::select_provider`
    // (unit-tested). Once a provider is configured (Settings → Providers), the
    // credential subscriber swaps it in at runtime — no restart — and a later
    // boot finds it here.
    let model_is_mock = model == "mock";
    let boot_without_provider = lucidos_engine::llm::boot_without_provider_enabled();

    if !model_is_mock {
        if project_id.is_empty() {
            log!("[Startup] Vertex AI not configured — Claude/Gemini models unavailable");
        } else {
            log!("[Startup] Vertex AI configured (project: {})", project_id);
        }
    }

    // Create the Vertex token cache up front so the SAME handle is shared by the
    // boot provider (via the build context), the engine (MemoryExtractor), and
    // the credential subscriber's rebuilds — reusing warm access tokens. `Some`
    // exactly when a real Vertex build is possible (non-mock + project
    // configured), matching the old per-selection storage.
    let vertex_token_cache: Option<lucidos_engine::llm::vertex::TokenCache> = (!model_is_mock
        && !project_id.is_empty())
    .then(|| std::sync::Arc::new(std::sync::Mutex::new(None)));

    // One throwaway pool for the initial registry load + provider build — the
    // engine's own pool doesn't exist until `LucidosEngine::new` below. `None`
    // (mock, or DB unavailable) degrades to env-only providers + an empty
    // registry, exactly as the prior `load_direct_providers_and_registry` did.
    let boot_pool = if model_is_mock {
        None
    } else {
        match sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(&database_url)
            .await
        {
            Ok(p) => Some(p),
            Err(e) => {
                log!(
                    "[Startup] Could not open pool for direct-provider credentials / model registry: {}",
                    e
                );
                None
            }
        }
    };
    if let Some(pool) = &boot_pool {
        let registry_map = lucidos_engine::llm::model_registry::load_from_db(pool).await;
        if let Ok(mut guard) = model_registry.write() {
            *guard = registry_map;
        }
    }

    let provider_ctx = ProviderBuildContext {
        default_model: model,
        model_is_mock,
        vertex_project_id: project_id.clone(),
        vertex_location: vertex_region.clone(),
        vertex_token_cache: vertex_token_cache.clone(),
        model_registry: model_registry.clone(),
        boot_without_provider,
    };
    let (llm, web_search) = match build_active_provider(boot_pool.as_ref(), &provider_ctx).await? {
        ProviderBuildOutcome::Install {
            llm,
            web_search,
            selection,
        } => {
            log!("[Startup] LLM provider installed: {:?}", selection);
            (llm, web_search)
        }
        ProviderBuildOutcome::FailFast => {
            panic!("No LLM provider configured. Set VERTEX_PROJECT_ID (Claude/Gemini via Vertex), configure an OpenAI / Anthropic / OpenRouter credential (Settings → Models → Providers) or OPENAI_API_KEY / LUCIDOS_OPENROUTER_API_KEY, set a local OpenAI-compatible base URL (Settings → Models → Providers or LUCIDOS_LOCAL_BASE_URL), LUCIDOS_BOOT_WITHOUT_PROVIDER=1 (boot into provider onboarding), or LUCIDOS_MODEL=mock (for testing).");
        }
    };
    drop(boot_pool);

    // Create engine with pgvector for embeddings.
    //
    // Deliberately NO catch-all boot-failure report here. A reported failure is
    // terminal — it stops the gateway respawning — and construction can fail for
    // plenty of transient reasons (Postgres not ready yet, a dropped connection
    // during schema init or recovery) that the supervisor's existing retry
    // recovers from. Only a CLASSIFIED-terminal failure reports, from the site
    // that can classify it: see `boot_failure::terminal_migration_message`.
    let engine = LucidosEngine::new(
        workspace_path.clone(),
        &database_url,
        llm,
        web_search,
        project_id,
        vertex_region,
        vertex_token_cache,
        model_registry,
    )
    .await?;
    log!("[Startup] PostgreSQL connected");

    // Retire the legacy per-workspace `data/.env`: import its KEY=VALUE lines
    // into the environment_variables table and delete the file. Runs once the DB
    // + migrations are up; self-idempotent (the file is gone afterwards).
    lucidos_engine::core::environment_variables::migrate_env_file_to_db(
        engine.pool(),
        &engine.event_bus,
        &workspace_path,
    )
    .await;
    // Apply stored env vars to the engine's OWN process env so engine-internal
    // shell-outs that inherit it (the engine's git push/fetch/clone, MCP
    // servers, …) see them — restoring what `data/.env` used to provide. Runs
    // after the migration so just-migrated vars are included. Reserved names are
    // filtered, so engine-critical process vars are never clobbered.
    lucidos_engine::core::environment_variables::apply_to_process_env(engine.pool()).await;

    // Extract shared read-only resources before wrapping engine in Arc
    let event_store = engine.event_store().clone();
    let embedder = engine.embedder().clone();
    let memory_index = engine.memory_index().clone();

    let shared_engine: SharedEngine = Arc::new(engine);
    shared_engine.set_self_arc(&shared_engine);
    shared_engine.start_parent_callback_listener();
    shared_engine.start_apply_all_driver();
    // The slot boots empty — load the embedding model in the background (tries
    // immediately, backs off on a fetch failure) and install it live once it
    // lands, so boot never waits on the multi-hundred-MB model download/load.
    shared_engine.spawn_embedder_load();

    // Construction is done (migrations only — the embedding model loads in the
    // background); the recovery sweeps below run before the HTTP server binds, so
    // narrate them on the boot splash.
    lucidos_engine::boot_report::report(lucidos_engine::boot_report::RECOVERING);

    // Acquire the engine startup lease BEFORE any reset/recovery below. A respawn
    // is not atomic — the gateway spawns this engine before the previous one has
    // exited (`respawn_stack` reaps the old child in a detached task) — so without
    // this the recovery sweeps would run against a DB the old engine is still
    // mutating: reading state before its device-attributed switch-abort boundary
    // lands (auto-resume misfires → manual Continue) and before its interrupted
    // Claude Code subprocesses finish draining (their late no-actor activity
    // events re-project threads to a phantom `running`). The lease is a
    // per-database advisory lock the previous engine holds until its process
    // exits (graceful: through its abort + CC-drain steps; crash: released
    // instantly when its connection dies), so this call blocks until the
    // predecessor is gone and recovery runs against a quiescent DB. Held as a
    // `run()` local for this engine's entire lifetime — dropped only when `run()`
    // returns (after our own graceful shutdown), releasing it for OUR successor.
    // Fail-open + bounded (see `startup_lease`): a hung predecessor degrades to
    // today's behavior rather than wedging boot. See
    // `docs/plans/2026-07-01-engine-startup-lease-recovery-race.md`.
    let _startup_lease = lucidos_engine::engine::startup_lease::acquire_startup_lease(
        &database_url,
        lucidos_engine::engine::startup_lease::DEFAULT_MAX_WAIT,
    )
    .await;

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
    // Same for the chat MCP permission lane: a chat turn parked on an
    // McpPermissionRequested is dead after restart, so clear the card.
    lucidos_engine::engine::mcp_permission::recover_orphan_mcp_permission_requests(
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
    // After restart, coding-agent sessions are dead — threads with no pending proposal
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
    if let Err(e) = lucidos_engine::engine::event_bus::EventBus::rebuild_blocking_descendant_count(
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
    // it's a no-op — `may_touch_change_state_at_idle` already fires for every
    // clean idle — so it only does work on the restart that lands this change
    // (and any future anomaly). See the function docstring for the per-thread
    // eligibility checks.
    lucidos_engine::engine::agent_recovery::propose_held_back_changes_on_startup(
        shared_engine.pool(),
        &shared_engine.event_bus,
        shared_engine.workspace_path(),
    )
    .await;

    // The mirror of the sweep above: a pending change that still EXISTS but
    // whose branch diff has since gone empty. Its card keeps advertising files
    // (and a restart) that the Diff button no longer shows. The live idle path
    // reconciles these as they happen; this catches rows that went stale while
    // no session was running. Runs after the two sweeps above so it sees the
    // rows they may have just created/refreshed, and before the HTTP server so
    // the first SSE payload is already honest.
    lucidos_engine::engine::agent_recovery::reconcile_emptied_changes_on_startup(&shared_engine)
        .await;

    // Re-deliver parent-resume wakes lost to the restart (ADR 0011, B1). The
    // in-memory ParentCallback channel was recreated empty above, so any blocking
    // child that completed while the engine was down — or whose wake was queued
    // but not consumed when it died — left a persisted ChildThreadCompleted on
    // its parent with no resume fired. This sweep re-injects those wakes onto the
    // same channel `start_parent_callback_listener` (started above) is already
    // draining, so the parent resumes through the identical live path. Runs after
    // the orphan/has-diff/held-back recovery so a parent that was mid-resume when
    // the engine died is recovered by CC auto-resume, not double-fired here.
    shared_engine
        .event_bus
        .refire_unprocessed_child_completions()
        .await;

    // Rebuild the Apply-All batch registry from the durable `apply_all_batches`
    // table and resolve any batch the previous process abandoned mid-flight
    // (an earlier member required a restart, or a conflict-resolution apply
    // landed a restart-requiring change). Runs AFTER agent/CC recovery above so
    // a member with an auto-resuming session is observed as running rather than
    // re-driven. Without this the in-memory registry came back empty, the
    // eventual terminal event found no batch, ApplyAllBatchCompleted was never
    // emitted, and the frontend's "Applying changes…" toast stuck forever.
    shared_engine.recover_apply_all_batches().await;

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

    // Auto-resume the coding-agent threads recovery flagged for a user-initiated
    // Switch to new version. Emitted HERE — after `SpawnDispatcher::spawn()` above,
    // which opens its broadcast subscription SYNCHRONOUSLY before returning, so
    // these emits are buffered even while the dispatcher's startup backfill is
    // still running (recovery ran before the dispatcher existed, so the emit can't
    // happen inside recovery; and a subscription acquired only after the backfill
    // would race these emits — the lost-ContinuationRequested zombie bug). The
    // dispatcher's startup orphan re-dispatch is the durable floor if an emit is
    // ever lost anyway. Crash-interrupted threads were NOT queued (they keep the
    // manual Continue affordance), so this never re-runs work that may have
    // crashed the engine.
    shared_engine.resume_pending_switches().await;

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

    // Chat parity of `resume_pending_switches` above: auto-resume the chat /
    // trigger threads a user-initiated Switch to new version interrupted. Same
    // cause gate as the coding-agent half (a device-attributed EngineShutdown
    // teardown abort), so a crash still falls back to the manual Continue.
    //
    // Deliberately LATER than the coding-agent drain. A coding-agent resume only
    // needs the spawn dispatcher subscribed; a chat resume re-enters the agentic
    // loop directly and immediately reads as `running`, so it must come after
    // `spawn_settle_subscriber()` above or that status change never reconciles a
    // Thread Queue slot. It must also follow `recover_orphan_tool_calls` (far
    // above), or the re-entered turn rebuilds an unpaired `tool_use` block and the
    // provider rejects the call.
    shared_engine.resume_pending_chat_switches().await;

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
    // Dev-only: periodically advance this engine's served-frontend snapshot to the
    // checkout-shared `dist/` when INV-A-safe, so a PEER workspace picks up another
    // workspace's frontend-only Apply without a manual restart (the applying engine
    // only advances its OWN snapshot). Spawned after `create_router` so the served
    // handle (`init_served_frontend`) is registered before the first tick. No-op in
    // packaged / headless. See
    // `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`.
    let _served_frontend_sync = shared_engine.spawn_served_frontend_sync();
    // Keep `/api/v1/health`'s `database_reachable` honest. An engine outlives its
    // database (quitting Docker Desktop is the everyday case in dev), and without
    // this the endpoint would keep reporting a healthy engine while every other
    // request failed. Not dev-only: a packaged install's bundled Postgres can die
    // too. See `engine::db_health` and ADR 0037.
    let _db_health_probe = shared_engine.spawn_db_health_probe();
    let api_port = std::env::var("LUCIDOS_API_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3000);
    // SECURITY: resolve the bind with a loopback-first precedence (see
    // `net_config`): the behind-gateway floor (`LUCIDOS_BIND_LOOPBACK`) → an
    // explicit `LUCIDOS_BIND_ADDR` → `LUCIDOS_BIND_ALL` → the machine + per-
    // workspace config (`~/.lucidos/network.toml` `[engine] inherit` picks the
    // gateway bind vs this workspace's `network_bind` preference) → loopback.
    // Default with nothing set is loopback-only (a directly-launched engine has
    // no request-level API auth); a malformed value fails safe to loopback,
    // never to all-interfaces. `[::]` (dual-stack) is used for the all-interfaces
    // case — macOS defaults IPV6_V6ONLY=0 so it serves IPv4 too. A packaged
    // gateway engine sets `LUCIDOS_BIND_LOOPBACK=1` (the `behind_gateway`
    // signal), pinning it to loopback while the gateway proxies it over
    // `http://127.0.0.1:<port>`.
    let loopback_signal = std::env::var("LUCIDOS_BIND_LOOPBACK")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let bind_addr_env = std::env::var("LUCIDOS_BIND_ADDR").ok();
    let bind_all_env = std::env::var("LUCIDOS_BIND_ALL").ok();
    let net = net_config::read_network_toml();
    // The per-workspace bind only matters when engines do NOT inherit the
    // gateway bind; read it from this workspace's DB then.
    let per_workspace_bind = if !loopback_signal && !net.engine_inherit {
        lucidos_engine::core::preferences::PreferenceStore::get(
            shared_engine.pool(),
            net_config::NETWORK_BIND_PREF_KEY,
        )
        .await
        .ok()
        .flatten()
    } else {
        None
    };
    let bind_choice = net_config::resolve_engine_bind(
        loopback_signal,
        bind_addr_env.as_deref(),
        bind_all_env.as_deref(),
        net.engine_inherit,
        net.gateway_bind.as_deref(),
        per_workspace_bind.as_deref(),
    );
    // Every address to listen on. A specific `Address` ALSO binds loopback (see
    // `net_config::bind_socket_addrs`) so the gateway proxy/health probe, the dev
    // scripts, and the engine's own restart callback — all over `127.0.0.1` — keep
    // working. `addr` (the primary) is used only for the startup log; `bind_label`
    // already notes the retained loopback.
    let addrs = net_config::bind_socket_addrs(&bind_choice, api_port);
    let addr = addrs[0];
    let bind_label = net_config::bind_scope_label(&bind_choice);

    let handle = axum_server::Handle::new();
    tokio::spawn(shutdown_signal(
        handle.clone(),
        shared_engine.clone(),
        scheduler,
    ));

    // Detect TLS certs — if present, serve HTTPS with HTTP/2. The http/https
    // decision is resolved in ONE place (`net_config::tls_scheme_from`); the
    // `if let` below only loads the cert paths that decision implies.
    let tls_cert = std::env::var("LUCIDOS_TLS_CERT").ok();
    let tls_key = std::env::var("LUCIDOS_TLS_KEY").ok();
    let scheme = net_config::tls_scheme_from(tls_cert.as_deref(), tls_key.as_deref());

    // Serve every resolved address concurrently, sharing the one graceful-shutdown
    // `Handle` so a single shutdown stops all sockets. A bind failure on any
    // address fails fast (same as the prior single-bind semantics).
    if scheme == net_config::SCHEME_HTTPS {
        // `tls_scheme_from` returned https ⇒ both paths are present and non-empty.
        let (cert_path, key_path) = (tls_cert.unwrap_or_default(), tls_key.unwrap_or_default());
        let tls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path).await?;
        log!(
            "[Startup] API server listening on {}://{}  (HTTP/2 + TLS, {})",
            scheme,
            addr,
            bind_label
        );
        futures::future::try_join_all(addrs.into_iter().map(|a| {
            axum_server::bind_rustls(a, tls_config.clone())
                .handle(handle.clone())
                .serve(app.clone().into_make_service())
        }))
        .await?;
    } else {
        log!(
            "[Startup] API server listening on {}://{}  ({})",
            scheme,
            addr,
            bind_label
        );
        futures::future::try_join_all(addrs.into_iter().map(|a| {
            axum_server::bind(a)
                .handle(handle.clone())
                .serve(app.clone().into_make_service())
        }))
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
        let deeper = if depth == 0 {
            0
        } else {
            consume_stack(depth - 1)
        };
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
        assert_eq!(
            sum, expected,
            "deep recursion ran every level to completion"
        );
    }
}
