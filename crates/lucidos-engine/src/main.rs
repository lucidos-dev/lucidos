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

    // Legitimate stop signal. SIGUSR1 rather than SIGTERM, so a stray `kill`
    // from a coding-agent subprocess test cannot take the engine down. Every
    // legitimate stop path (web-dev.sh, stop.sh, /api/v1/restart) sends SIGUSR1.
    #[cfg(unix)]
    let usr1 = async {
        signal::unix::signal(signal::unix::SignalKind::user_defined1())
            .expect("failed to install SIGUSR1 handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let usr1 = std::future::pending::<()>();

    // SIGTERM ignorer. With no installed handler the kernel takes the default
    // action, so a test script that `xargs kill`s the engine's pid still kills
    // it. This handler logs and loops instead. Leaked deliberately: it must
    // live for the rest of the process.
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

    // Open the teardown. Two things happen here, and both must happen before
    // any cleanup event is emitted:
    //
    // 1. The engine is marked shutting down, which stops the scheduler firing
    //    event-triggers and stops an event-wait resolution opening a new turn.
    //    The sweeps below emit terminator events that fan out to triggers, and
    //    a trigger script's callback would then hit the HTTP API being torn
    //    down. The scheduler's own shutdown flag is set far later, too late to
    //    gate these events.
    // 2. The teardown's ACTOR is decided once, for every emit inside it. A
    //    device actor means a user asked for this, and yields "You"
    //    attribution plus auto-resume on the next boot. `None` means nobody
    //    did, and yields System attribution plus a manual Continue.
    let teardown_actor = engine.begin_teardown();

    // Emit the boundary "Switched to new version" and abort events at real
    // teardown, never at switch-request time. Nothing may show "Switched" while
    // the old engine is still alive through a dev rebuild.
    //
    // Runs BEFORE the session sweeps, so its `external_terminal_emitted` flags
    // suppress their duplicate emits. Those sweeps read the same actor back off
    // the engine, so a thread that became in-flight after this snapshot gets
    // the same verdict.
    engine.abort_in_flight_for_restart(teardown_actor).await;

    // Record every background task this process is about to kill, while its
    // buffered output still exists. `is_shutting_down` already holds, set by
    // `begin_teardown` above, so the event-wait dispatcher declines to open a
    // turn on an engine that is leaving.
    //
    // AFTER the boundary emits, not before, because this one blocks: it kills
    // each child and waits for the reap. The supervisor force-kills the whole
    // shutdown at 15 s, and a device-attributed `ResponseAborted` that misses
    // that window costs every in-flight thread its auto-resume. The tail of a
    // background log is not worth that, so the durable emits go first.
    //
    // The boot sweep is the floor under this, for the deaths no hook sees.
    engine.settle_running_background_tasks_at_teardown().await;

    // Must happen before HTTP shutdown, so the event bus can still persist the
    // CodingAgentIdled events that carry each session id.
    engine.shutdown_agent_sessions().await;
    engine.shutdown_active_threads().await;

    handle.graceful_shutdown(Some(Duration::from_secs(10)));

    // Avoid orphaning Chrome.
    engine.shutdown_browser().await;

    // Same reason, for the frontend preview's Vite. It has its own process
    // group, so nothing else on this path reaches it, and a leaked one holds
    // its port against the successor's preview.
    engine.stop_frontend_preview().await;

    if let Err(e) = scheduler.lock().await.shutdown().await {
        log!("[Shutdown] Error shutting down scheduler: {}", e);
    }

    log!("[Shutdown] Shutdown complete.");
}

/// Postgres `undefined_table`. This read runs before `LucidosEngine::new` has
/// applied the migrations, so a brand-new workspace has no `preferences` table
/// on its first boot.
const PG_UNDEFINED_TABLE: &str = "42P01";

/// The stored `vertex_region`, or `None` when no row holds one.
///
/// `Err` means the read could not run, which is NOT the same answer as `None`.
/// A cold Postgres can time out the 5s acquire, and the caller then falls back
/// to a region the user never chose. It says which happened rather than
/// reporting the setting as absent.
///
/// A missing table is the one failure that IS `None`: no table means no stored
/// setting, and warning about it would fire on every first boot.
async fn read_vertex_region_pref(
    database_url: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await?;
    let stored = sqlx::query_scalar::<_, String>(
        "SELECT value FROM preferences WHERE key = $1 AND device_id IS NULL",
    )
    .bind(lucidos_engine::core::PREF_VERTEX_REGION)
    .fetch_optional(&pool)
    .await;
    match stored {
        Ok(value) => Ok(value.filter(|s| !s.is_empty())),
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some(PG_UNDEFINED_TABLE) => {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// The Vertex region to use when no stored setting supplies one, and the name
/// of the input that supplied it.
fn vertex_region_from_env_or_default() -> (String, &'static str) {
    match std::env::var("VERTEX_REGION")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(region) => (region, "VERTEX_REGION"),
        None => (
            lucidos_engine::core::DEFAULT_VERTEX_REGION.to_string(),
            "the default",
        ),
    }
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
/// `git` is a hard dependency of the coding-agent, Apply and worktree flows,
/// but not of chat. So a missing or broken `git` is a loud, actionable warning
/// at boot, never a fatal exit: aborting would brick a chat-only packaged
/// install on a Mac with no Xcode Command Line Tools, and respawn-loop it under
/// launchd. On the launchd minimal PATH `/usr/bin/git` is the
/// Command-Line-Tools shim, which ENOENTs when CLT is absent.
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

/// One-time `python3 --version` boot preflight, [`git_preflight`]'s sibling.
///
/// `python3` backs the `run_python` tool and nothing else, so a missing
/// interpreter is a loud boot warning rather than a fatal exit. Same
/// Command-Line-Tools shim story as git.
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
/// trigger fire descends the scheduler, the Thread Queue executor, the agentic
/// loop, `execute_intent`, a nested sub-loop and a tool, all as one un-spawned
/// future. Tokio's default 2 MiB worker stack overflows on that depth and
/// aborts the engine.
///
/// The depth is bounded, because `execute_intent` is excluded from the intent
/// sub-loop's tool set, so a larger fixed stack resolves it. 16 MiB is reserved
/// virtual address space committed lazily per touched page, so the headroom is
/// effectively free.
const WORKER_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

/// Build the multi-threaded Tokio runtime the engine runs on. What
/// `#[tokio::main]` produces, plus a larger worker-thread stack (see
/// [`WORKER_THREAD_STACK_SIZE`]). Shared with the runtime test, so it exercises
/// the exact production config.
fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_THREAD_STACK_SIZE)
        .build()
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Handled before any heavy startup. Bare `println!` rather than `log!`,
    // because tooling parses this and the `log!` prefix would break callers.
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

    // A running engine forks the on-disk binary with this flag to compare build
    // ids, which is the dev "new version available" check. Bare `println!`:
    // machine-read stdout, so no `log!` prefix.
    if std::env::args().skip(1).any(|a| a == "--build-id") {
        println!("{}", lucidos_engine::ENGINE_BUILD_ID);
        return Ok(());
    }

    // Packaging gate, run by the DMG build against the just-signed binary. It
    // compiles a tiny Wasm module and calls it. That call is what the macOS
    // hardened runtime kills when the engine may not execute code it compiled
    // itself. Exits before any engine construction, so it needs no database, no
    // workspace and no network.
    if std::env::args().skip(1).any(|a| a == "--wasm-selftest") {
        return match build_runtime()?.block_on(lucidos_engine::jit_selftest()) {
            Ok(()) => {
                log!("[Selftest] wasm JIT selftest passed");
                Ok(())
            }
            Err(e) => Err(format!("wasm JIT selftest failed: {e}").into()),
        };
    }

    // The workspace gateway is the standalone `lucidos-gateway` binary (ADR
    // 0014 §1). A stray `--gateway` flag is a misconfiguration: fail loudly,
    // rather than silently booting one engine on the gateway's port.
    if std::env::args().skip(1).any(|a| a == "--gateway") {
        return Err("`--gateway` was removed from lucidos-engine; run the \
                    `lucidos-gateway` binary instead (ADR 0014)"
            .into());
    }

    // One-shot restore subcommand. The gateway cannot link the engine crate
    // (ADR 0014 §1). It shells out to this binary instead, to restore a local
    // backup archive into a workspace it already provisioned. The engine it
    // then spawns runs migrations at construction, upgrading an older-schema
    // restore.
    if std::env::args().nth(1).as_deref() == Some("restore-archive") {
        return build_runtime()?.block_on(run_restore_archive());
    }

    build_runtime()?.block_on(run())
}

/// `lucidos-engine restore-archive --file <enc> --workspace-dir <dir>`.
///
/// Restores a local encrypted backup archive into an already-provisioned
/// workspace directory and database.
///
/// The key and the connection string both arrive through the environment, so
/// neither lands in argv, which `ps` shows. Prints
/// `LUCIDOS_RESTORE_PHASE=<phase>:<pct>` lines so the gateway can show coarse
/// progress in the picker, and exits non-zero with the error on stderr.
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
            // One progress line per tick, best-effort: the gateway parses the
            // latest phase for the picker's restore status.
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "LUCIDOS_RESTORE_PHASE={phase}:{cur}");
            let _ = out.flush();
        },
    )
    .await?;
    Ok(())
}

/// What this engine will bind, resolved from env, `network.toml` and the
/// per-workspace preference.
///
/// SECURITY: the bind resolves loopback-first, and `net_config` owns the whole
/// precedence order. With nothing set the default is loopback-only. A malformed
/// value fails safe to loopback, never to all-interfaces.
///
/// The all-interfaces case binds `[::]`, and macOS defaults IPV6_V6ONLY=0 so
/// that serves IPv4 too. A packaged gateway engine sets
/// `LUCIDOS_BIND_LOOPBACK=1`, pinning it to loopback behind the proxy.
///
/// Called before the router is built, because the answer decides whether that
/// router requires a credential (`api::local_auth`). A wide bind no longer
/// means an unauthenticated API. It does mean a browser must come through the
/// gateway rather than straight to this port.
async fn resolve_bind_choice(engine: &SharedEngine) -> net_config::BindChoice {
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
            engine.pool(),
            net_config::NETWORK_BIND_PREF_KEY,
        )
        .await
        .ok()
        .flatten()
    } else {
        None
    };
    net_config::resolve_engine_bind(
        loopback_signal,
        bind_addr_env.as_deref(),
        bind_all_env.as_deref(),
        net.engine_inherit,
        net.gateway_bind.as_deref(),
        per_workspace_bind.as_deref(),
    )
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Before any TLS use, and before anything that might reach the network.
    lucidos_engine::net_config::install_crypto_provider();

    let _ = dotenvy::dotenv();

    // macOS defaults to 256, too low for concurrent coding-agent sessions, SSE
    // streams, the DB pool and static file serving.
    raise_fd_limit();

    log!("[Startup] Lucidos Engine starting...");

    // So a post-mortem can verify the supervisor chain from the log alone. The
    // `log!` macro already prepends this process's own pid.
    #[cfg(unix)]
    {
        // SAFETY: getppid / getpgrp / getsid are async-signal-safe and take no
        // pointer arguments, so calling them is well-defined.
        let ppid = unsafe { libc::getppid() };
        let pgid = unsafe { libc::getpgrp() };
        let sid = unsafe { libc::getsid(0) };
        log!("[Startup] ppid={} pgid={} sid={}", ppid, pgid, sid);
    }

    // Prepend the common user-install bin dirs before anything spawns or probes
    // a tool. A packaged engine inherits its service manager's minimal PATH,
    // which ENOENTs bare-name tools that resolve fine in a dev shell.
    lucidos_engine::core::user_path::augment_process_path();

    git_preflight();
    python_preflight();

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

    // Point the embedding-model cache at the shared per-user directory unless
    // something already chose one. See ADR 0061. Runs BEFORE
    // `apply_to_process_env` below, so a value the user stored in Settings
    // still wins.
    lucidos_engine::memory::apply_default_cache_dir(&workspace_path);

    // Upgrade a legacy `apis.json` to the pipeline shape. Idempotent, and it
    // writes a backup first. NOT fatal: an `Err` here is the filesystem or
    // the JSON parser saying no, and the config load below reports what it
    // then refuses. See ADR 0135.
    match lucidos_engine::api::proxy_migration::migrate_apis_json_if_needed(&workspace_path) {
        Ok(lucidos_engine::api::proxy_migration::MigrationOutcome::Migrated { backup_path }) => {
            log!(
                "[Startup] migrated apis.json to pipeline shape (backup at {})",
                backup_path.display()
            );
        }
        Ok(_) => {}
        Err(e) => log!("[Startup] apis.json migration skipped: {}", e),
    }

    // Read the config once here so a refused entry is named in the startup
    // log, then hand the list on. The proxy routes re-read the same file per
    // request, so this call adds no authority, only timing. The announce
    // cannot happen yet: there is no EventBus until `LucidosEngine::new`.
    let proxy_load = lucidos_engine::api::load_proxy_config(&workspace_path);
    // Trust on first sight, once per workspace: a handshake that worked before
    // ADR 0144 keeps working, and every later start refuses a script nobody
    // authored. `seed_if_absent` will not run twice.
    let seeded_handshake_scripts = match lucidos_engine::core::handshake_approvals::seed_if_absent(
        &workspace_path,
        &proxy_load.handshake_seed_entries(),
    ) {
        Ok(seeded) => seeded,
        Err(e) => {
            log!("[Startup] could not seed handshake approvals: {}", e);
            Vec::new()
        }
    };
    for path in &seeded_handshake_scripts {
        log!(
            "[Startup] approved the handshake script already in use: {}",
            path
        );
    }
    // Read before `rejected` is moved out below: only the map is kept, not the
    // whole config.
    let configured_credential_scopes = proxy_load.credential_scopes();
    let proxy_config_rejections = proxy_load.rejected;
    for rejected in &proxy_config_rejections {
        log!(
            "[Startup] apis.json: refusing '{}': {}",
            rejected.label(),
            rejected.reason
        );
    }
    let proxy_insecure_transports = proxy_load.insecure;
    for insecure in &proxy_insecure_transports {
        log!(
            "[Startup] apis.json: '{}' has an unauthenticated transport: {}",
            insecure.provider,
            insecure.reason
        );
    }

    let database_url = lucidos_engine::core::database_url();
    log!("[Startup] Connecting to PostgreSQL...");

    // Resolve the Vertex project WITHOUT requiring the `gcloud` binary, so a
    // packaged build works from the user's existing ADC. The subprocess call is
    // the last resort.
    let project_id = std::env::var("VERTEX_PROJECT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(lucidos_engine::llm::vertex::adc::project_from_files)
        .or_else(get_gcloud_project)
        .unwrap_or_default();

    // The log line below names which input won. Nothing else tells a user who
    // set the region in Settings why requests still go somewhere else. An
    // exported VERTEX_REGION only counts if this process inherited it.
    let (initial_region, region_source) = match read_vertex_region_pref(&database_url).await {
        Ok(Some(region)) => (region, "the vertex_region setting"),
        Ok(None) => vertex_region_from_env_or_default(),
        Err(e) => {
            // Falls back exactly as an absent setting does, but says so. A
            // silent fallback here reads as "you never set a region".
            log!(
                "[Startup] WARNING: could not read the vertex_region setting ({}), \
                 falling back",
                e
            );
            vertex_region_from_env_or_default()
        }
    };
    let vertex_region = lucidos_engine::llm::vertex::location_handle(initial_region.clone());

    let model = std::env::var("LUCIDOS_MODEL")
        .unwrap_or_else(|_| lucidos_engine::core::DEFAULT_CHAT_MODEL.to_string());

    log!("[Startup] Using default model: {}", model);

    // Shared model→provider routing map. Populated from the DB below for real
    // providers; left empty for mock. Cloned into RoutingProvider and handed to
    // the engine so the reload subscriber can hot-swap it on Model* events.
    let model_registry = lucidos_engine::llm::model_registry::empty();

    // Built through `llm::build_active_provider`, so this boot path and the
    // runtime credential subscriber produce an identical provider. The decision
    // matrix itself lives in `llm::select_provider`, which is unit-tested.
    //
    // Once a provider is configured, the credential subscriber swaps it in
    // without a restart, and a later boot finds it here.
    let model_is_mock = model == "mock";
    let boot_without_provider = lucidos_engine::llm::boot_without_provider_enabled();

    if !model_is_mock {
        if project_id.is_empty() {
            log!("[Startup] Vertex AI not configured — Claude/Gemini models unavailable");
        } else {
            log!(
                "[Startup] Vertex AI configured (project: {}, region: {} from {})",
                project_id,
                initial_region,
                region_source
            );
        }
    }

    // Created up front so the SAME handle reaches the boot provider, the engine
    // and the credential subscriber's rebuilds, reusing warm access tokens.
    // `Some` exactly when a real Vertex build is possible.
    let vertex_token_cache: Option<lucidos_engine::llm::vertex::TokenCache> = (!model_is_mock
        && !project_id.is_empty())
    .then(|| std::sync::Arc::new(std::sync::Mutex::new(None)));

    // One throwaway pool for the initial registry load and provider build: the
    // engine's own pool does not exist until `LucidosEngine::new` below. `None`
    // degrades to env-only providers and an empty registry.
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
            panic!("No LLM provider configured. Set VERTEX_PROJECT_ID (Claude/Gemini via Vertex), configure an OpenAI / Anthropic / OpenRouter / xAI credential (Settings → Models → Providers) or OPENAI_API_KEY / ANTHROPIC_API_KEY / LUCIDOS_OPENROUTER_API_KEY / LUCIDOS_XAI_API_KEY, set a local OpenAI-compatible base URL (Settings → Models → Providers or LUCIDOS_LOCAL_BASE_URL), turn on the keyless OpenCode Free tier (Settings → Models → Providers or LUCIDOS_OPENCODE_FREE=1), LUCIDOS_BOOT_WITHOUT_PROVIDER=1 (boot into provider onboarding), or LUCIDOS_MODEL=mock (for testing).");
        }
    };
    drop(boot_pool);

    // Deliberately NO catch-all boot-failure report here. A reported failure is
    // terminal and stops the gateway respawning, while construction fails for
    // plenty of transient reasons the supervisor's retry recovers from. Only a
    // CLASSIFIED-terminal failure reports, from the site that can classify it:
    // see `boot_failure::terminal_migration_message`.
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

    // Retire the legacy per-workspace `data/.env` into the
    // environment_variables table. Needs the DB and its migrations, and is
    // self-idempotent because the file is gone afterwards.
    lucidos_engine::core::environment_variables::migrate_env_file_to_db(
        engine.pool(),
        &engine.event_bus,
        &workspace_path,
    )
    .await;
    // Apply stored env vars to the engine's OWN process env, so every
    // engine-internal shell-out that inherits it sees them. Runs after the
    // migration, so just-migrated vars are included. Reserved names are
    // filtered, so engine-critical process vars are never clobbered.
    lucidos_engine::core::environment_variables::apply_to_process_env(engine.pool()).await;

    // Tell the workspace about any `apis.json` entry the load refused above.
    // The read happened before the DB was up, so this is the first point with
    // an EventBus to announce it on. Silent when nothing was refused.
    //
    // TWO surfaces, because the SSE one cannot reach anybody from here. This
    // runs long before `axum_server::bind`, and `/api/v1/events` hands a client
    // the LIVE broadcast with no replay. So the event alone is sent to zero
    // subscribers on every ordinary boot. The notification is the guaranteed
    // half: the bus projects it into `notifications`, which the client polls
    // whenever it connects. See ADR 0135.
    if !proxy_config_rejections.is_empty() {
        let summary = proxy_config_rejections
            .iter()
            .map(|r| format!("{}: {}", r.label(), r.reason))
            .collect::<Vec<_>>()
            .join("\n");
        engine
            .event_bus
            .emit_or_log(
                lucidos_engine::engine::event_bus::BusEvent::System(
                    lucidos_engine::engine::event_bus::SystemEvent::NotificationCreated {
                        id: uuid::Uuid::new_v4().to_string(),
                        title: "Proxy config problem".to_string(),
                        message: format!(
                            "Lucidos will not serve these entries in \
                             data/config/apis.json, and a call to one answers 502. \
                             Everything else in the file still works.\n\n{summary}"
                        ),
                        task_id: None,
                        app_id: None,
                        thread_id: None,
                        event_id: None,
                        tap: Default::default(),
                        actor: None,
                    },
                ),
                "[Proxy] apis.json NotificationCreated",
            )
            .await;
        engine
            .event_bus
            .emit_or_log(
                lucidos_engine::engine::event_bus::BusEvent::System(
                    lucidos_engine::engine::event_bus::SystemEvent::ProxyConfigRejected {
                        rejected: proxy_config_rejections,
                    },
                ),
                "[Proxy] ProxyConfigRejected",
            )
            .await;
    }

    // Say once, loudly, which providers the engine will not vouch for. Same two
    // surfaces and same reasoning as the rejection announce above: nothing is
    // subscribed to SSE this early, so the notification is the half that lands.
    if !proxy_insecure_transports.is_empty() {
        let summary = proxy_insecure_transports
            .iter()
            .map(|i| format!("{}: {}", i.provider, i.reason))
            .collect::<Vec<_>>()
            .join("\n");
        engine
            .event_bus
            .emit_or_log(
                lucidos_engine::engine::event_bus::BusEvent::System(
                    lucidos_engine::engine::event_bus::SystemEvent::NotificationCreated {
                        id: uuid::Uuid::new_v4().to_string(),
                        title: "Proxy transport is not authenticated".to_string(),
                        message: format!(
                            "Anyone on the network path to these providers can read \
                             the credentials Lucidos sends them.\n\n{summary}"
                        ),
                        task_id: None,
                        app_id: None,
                        thread_id: None,
                        event_id: None,
                        tap: Default::default(),
                        actor: None,
                    },
                ),
                "[Proxy] insecure transport NotificationCreated",
            )
            .await;
    }

    // A credential the proxy would now refuse for want of a scope gets the one
    // its own entry uses, once, and says so (ADR 0144). Two entries disagreeing
    // is not an answer, so those are left for the user to settle.
    for (name, scopes) in configured_credential_scopes {
        let mut usable = scopes.iter().filter(|u| !u.trim().is_empty());
        let (Some(base_url), None) = (usable.next(), usable.next()) else {
            continue;
        };
        match lucidos_engine::core::CredentialStore::infer_scope_if_empty(
            engine.pool(),
            &engine.event_bus,
            &name,
            base_url,
        )
        .await
        {
            Ok(true) => log!("[Startup] scoped credential '{}' to {}", name, base_url),
            Ok(false) => {}
            Err(e) => log!("[Startup] could not scope credential '{}': {}", name, e),
        }
    }

    // The builtin `local` provider has no `apis.json` entry to read a scope
    // from, so its credential gets the base URL the provider itself resolves.
    // Same once-only inference, and it keeps a workspace that already stores a
    // `local` key working after the proxy started checking scopes.
    if let Some(local_base) = lucidos_engine::api::local_upstream_base_url(engine.pool()).await {
        match lucidos_engine::core::CredentialStore::infer_scope_if_empty(
            engine.pool(),
            &engine.event_bus,
            "local",
            &local_base,
        )
        .await
        {
            Ok(true) => log!("[Startup] scoped credential 'local' to {}", local_base),
            Ok(false) => {}
            Err(e) => log!("[Startup] could not scope credential 'local': {}", e),
        }
    }

    // Same two surfaces as the rejection announce above, and for the same
    // reason: nothing is subscribed to SSE this early, so the notification is
    // the half that reaches the user. Silent when nothing was seeded.
    if !seeded_handshake_scripts.is_empty() {
        for path in &seeded_handshake_scripts {
            engine
                .event_bus
                .emit_or_log(
                    lucidos_engine::engine::event_bus::BusEvent::System(
                        lucidos_engine::engine::event_bus::SystemEvent::HandshakeScriptApproved {
                            path: path.clone(),
                            source:
                                lucidos_engine::core::handshake_approvals::ApprovalSource::Seeded,
                            actor: None,
                        },
                    ),
                    "[Proxy] HandshakeScriptApproved",
                )
                .await;
        }
        engine
            .event_bus
            .emit_or_log(
                lucidos_engine::engine::event_bus::BusEvent::System(
                    lucidos_engine::engine::event_bus::SystemEvent::NotificationCreated {
                        id: uuid::Uuid::new_v4().to_string(),
                        title: "Approved your handshake scripts".to_string(),
                        message: format!(
                            "Lucidos now runs an auth handshake script only if it \
                             recorded who wrote it. Lucidos approved the scripts \
                             already in use as they stand:\n\n{}\n\nEdit one outside \
                             Lucidos and it stops running until you approve it again.",
                            seeded_handshake_scripts.join("\n")
                        ),
                        task_id: None,
                        app_id: None,
                        thread_id: None,
                        event_id: None,
                        tap: Default::default(),
                        actor: None,
                    },
                ),
                "[Proxy] handshake seed NotificationCreated",
            )
            .await;
    }

    // Extracted before the engine is wrapped in an Arc.
    let event_store = engine.event_store().clone();
    let embedder = engine.embedder().clone();
    let memory_index = engine.memory_index().clone();

    let shared_engine: SharedEngine = Arc::new(engine);
    shared_engine.set_self_arc(&shared_engine);
    shared_engine.start_parent_callback_listener();
    shared_engine.start_apply_all_driver();
    // The slot boots empty and the model is installed live once it lands, so
    // boot never waits on a multi-hundred-MB download.
    shared_engine.spawn_embedder_load();

    // The recovery sweeps below run before the HTTP server binds, so narrate
    // them on the boot splash.
    lucidos_engine::boot_report::report(lucidos_engine::boot_report::RECOVERING);

    // Acquired BEFORE any reset or recovery below. A respawn is not atomic. The
    // gateway spawns this engine before the previous one exits, so the sweeps
    // would otherwise run against a DB the old engine is still mutating.
    //
    // The lease is a per-database advisory lock. The predecessor holds it until
    // its process exits, so this call blocks until recovery can run against a
    // quiescent DB. Held as a `run()` local for this engine's whole lifetime,
    // and released for OUR successor when `run()` returns. Fail-open and
    // bounded, so a hung predecessor cannot wedge boot. See
    // `docs/plans/2026-07-01-engine-startup-lease-recovery-race.md`.
    let _startup_lease = lucidos_engine::engine::startup_lease::acquire_startup_lease(
        &database_url,
        lucidos_engine::engine::startup_lease::DEFAULT_MAX_WAIT,
    )
    .await;

    // A respawn sidecar means the previous engine died unexpectedly, so record
    // it in the audit timeline. Emits before recovery, so the timeline reads
    // "supervisor respawn, then recovery". A missing sidecar is the common case.
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
    // A voice session died with the process holding its socket, so every
    // unpaired start belongs to a call that is already over.
    lucidos_engine::voice::recovery::settle_orphan_voice_sessions(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;

    // Reset threads orphaned in 'running' by the previous engine process. The
    // recovery below sets the correct status.
    //
    // Scoped to 'running' ON PURPOSE. `waiting_for_user_answer` stays out of
    // it, because the user's answer is what resumes such a thread. A thread
    // holding an *event wait* needs no exemption: a subscription does not hold
    // a turn, so it is already `idle` here (ADR 0049).
    if let Err(e) =
        sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE status = 'running'")
            .execute(shared_engine.pool())
            .await
    {
        log!("[Startup] Failed to reset orphaned running threads: {}", e);
    }

    // Reset coding-agent threads stuck in 'waiting' with no pending proposal.
    // Their sessions are dead after a restart, so there is nothing for the user
    // to act on. Chat threads cannot reach 'waiting', so the
    // `source='claude_code'` scope is defensive.
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

    // Reconcile active_children_count in either drift direction. The query and
    // its reasons live with the in-tx reconcile it must agree with, so the two
    // cannot drift apart on what "in flight" means.
    if let Err(e) = lucidos_engine::engine::event_bus::EventBus::rebuild_active_children_count(
        shared_engine.pool(),
    )
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

    // Reconcile blocking_descendant_count. The two resets above move child rows
    // out of blocking states by direct UPDATE, bypassing the projection's
    // sampling wrapper, so no parent count is decremented.
    //
    // The later recovery sweep emits its terminators through EventBus, but
    // `prev_sample` already reads 'idle' by then and the projection computes a
    // zero delta. Without this reconciliation the parent's Archive button stays
    // disabled forever, and its cascade never runs.
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

    // Place this workspace in the release-notice sequence, once. Both inputs
    // are boot facts: the release this binary reports, and whether the
    // workspace has ever held a thread. A workspace already in the sequence is
    // left alone, so this is a no-op on every boot but the first.
    lucidos_engine::engine::release_notices::seed_cursor_at_startup(
        shared_engine.pool(),
        lucidos_engine::engine::release_notices::all(),
    )
    .await;

    // Started BEFORE the recovery sweep below. Recovery broadcasts terminators
    // this consumer must see to flip abandoned todos, and a tokio broadcast
    // channel does not replay history for a late subscriber.
    lucidos_engine::engine::todo_consumer::spawn(shared_engine.clone());

    // Recover worktrees whose in-flight session an engine crash interrupted. An
    // idle session stays idle, shown in the waiting UI for the user to act on.
    let recovering_threads = shared_engine.recover_orphaned_worktrees().await;
    shared_engine
        .recover_orphaned_threads(&recovering_threads)
        .await;

    // Recover orphan `ToolCalled` events, left by an engine that died mid-tool.
    // Without this the thread's next LLM call rebuilds an assistant `tool_use`
    // block whose pair is missing, and the provider rejects the request.
    shared_engine.recover_orphan_tool_calls().await;

    // Reconcile `thread_summaries.coding_agent_has_diff` against on-disk git
    // for every active coding-agent thread. Live updates flow through the
    // change projection handlers, but a mid-turn commit does not, so this
    // sweep is the authoritative reconciliation. It runs before the HTTP server
    // serves SSE, because the Diff button must reflect git from the first
    // connection.
    lucidos_engine::engine::agent_recovery::refresh_coding_agent_has_diff_on_startup(
        shared_engine.pool(),
        shared_engine.workspace_path(),
    )
    .await;

    // Propose changes never surfaced at idle: an idle coding-agent thread with
    // a committed branch diff but no pending change. A safety net for any
    // missed idle-proposal, and a no-op in steady state, because
    // `may_touch_change_state_at_idle` already fires for every clean idle.
    lucidos_engine::engine::agent_recovery::propose_held_back_changes_on_startup(
        shared_engine.pool(),
        &shared_engine.event_bus,
        shared_engine.workspace_path(),
    )
    .await;

    // The mirror of the sweep above: a pending change that still EXISTS but
    // whose branch diff has gone empty, so its card advertises files the Diff
    // button no longer shows. The live idle path handles these as they happen;
    // this catches rows that went stale while no session was running.
    //
    // Runs after the two sweeps above, so it sees rows they just refreshed, and
    // before the HTTP server, so the first SSE payload is honest.
    lucidos_engine::engine::agent_recovery::reconcile_emptied_changes_on_startup(&shared_engine)
        .await;

    // Re-deliver parent-resume re-entries lost to the restart (ADR 0011). The
    // in-memory channel was recreated empty above. So a blocking child that
    // completed while the engine was down left a persisted ChildThreadCompleted
    // with no resume fired.
    //
    // This re-injects those onto the channel `start_parent_callback_listener`
    // is already draining, so the parent resumes through the live path. Runs
    // after the recovery sweeps, so a parent mid-resume when the engine died is
    // recovered there rather than double-fired here.
    shared_engine
        .event_bus
        .refire_unprocessed_child_completions()
        .await;

    // The event-wait dispatcher (ADR 0047). Order inside this block is
    // load-bearing and each step is documented on its own method:
    //
    // 0. The re-entry consumer first of all: every path below hands its turn to
    //    this task rather than awaiting it, so nothing resumes until it runs.
    // 1. The subscriber next, so an event landing during the rebuild is either
    //    matched live or caught by a watermark scan. Started after, it could be
    //    missed by both.
    // 2. The lost-re-entry sweep BEFORE the rebuild, and this order is
    //    load-bearing rather than tidy. The sweep looks for a resolution whose
    //    only successor is its own anchor, which is exactly the shape the
    //    rebuild's catch-up scan *creates*: it persists the pair and hands the
    //    turn to a task that will not write anything for hundreds of
    //    milliseconds. Run after, the sweep re-drives every re-entry the rebuild
    //    just queued and each recovered thread runs two turns. Run first, it sees
    //    only the genuinely stranded resolutions from the previous process.
    // 3. The rebuild last: re-derive every live wait from the event store
    //    (there is no table) and run each catch-up scan. A match that landed
    //    while the engine was down then still reaches its thread. A deadline
    //    that passed expires loudly on the next sweep tick.
    // 4. The abandoned-background-task sweep between steps 2 and 3, for the
    //    reason stated at its own call site below.
    shared_engine.start_wait_reentry_consumer();
    shared_engine.start_event_wait_dispatcher();
    // Before either: close any `await_event` call the legacy attached-wait
    // shape left unpaired, or the thread 400s on its next turn. Ordered first,
    // so a wait that is ALSO re-armed below re-enters a thread whose message
    // array is already valid.
    shared_engine.settle_legacy_attached_event_waits().await;
    shared_engine.refire_unresolved_wait_reentries().await;

    // Settle the background tasks the last engine took down with it. A thread
    // that ended a turn with background work running is subscribed to that
    // task's `BackgroundBashCompleted`, and no process will ever emit it now.
    //
    // Wedged between the two steps around it, and both sides are load-bearing.
    // AFTER the lost-re-entry sweep, whose predicate is "the anchor is the last
    // event on this thread". This appends one, so running it first buries a
    // stranded re-entry and the thread never runs its turn. BEFORE the rebuild,
    // because its catch-up scan reads forward from each wait's watermark. That
    // scan is what re-opens the thread at boot instead of at its own deadline.
    //
    // The dispatcher above cannot match this live: the rebuild has not run, so
    // the cache holds no wait yet.
    shared_engine.settle_abandoned_background_tasks().await;

    shared_engine.rebuild_event_waits().await;

    // Rebuild the Apply-All batch registry from the durable table and resolve
    // any batch the previous process abandoned mid-flight. Runs AFTER the agent
    // recovery above, so a member with an auto-resuming session is observed as
    // running rather than re-driven.
    //
    // Without it the registry comes back empty, no batch matches the eventual
    // terminal event, and the frontend's "Applying changes" toast never clears.
    shared_engine.recover_apply_all_batches().await;

    lucidos_engine::engine::memory_consumer::spawn(shared_engine.clone());

    // See `engine::worktree_cleanup` module docs.
    lucidos_engine::engine::worktree_cleanup::WorktreeCleanup::spawn(
        shared_engine.pool().clone(),
        std::sync::Arc::new(shared_engine.event_bus.clone()),
        workspace_path.clone(),
        shared_engine.worktree_cleanup_active_threads(),
    );

    // The coding-agent spawn dispatcher. ContinuationRequested triggers are
    // production-active and push onto the dispatcher's outbound channel, which
    // `start_spawn_request_consumer` below drains. MessageReceived triggers
    // stay in SHADOW mode, because the chat HTTP handler still owns spawning
    // and its post-processing is not event-driven yet. See `spawn_dispatcher`
    // and `docs/plans/2026-04-24-cc-resume-spike-q7.md`.
    let (_dispatcher_handle, spawn_request_rx) =
        lucidos_engine::engine::spawn_dispatcher::SpawnDispatcher::spawn(
            shared_engine.pool().clone(),
            std::sync::Arc::new(shared_engine.event_bus.clone()),
        );
    shared_engine.start_spawn_request_consumer(spawn_request_rx);

    // Auto-resume the coding-agent threads recovery flagged for a user-initiated
    // Switch to new version.
    //
    // Emitted HERE, after `SpawnDispatcher::spawn()` opened its broadcast
    // subscription synchronously, so these emits are buffered even while its
    // startup backfill runs. Recovery itself is too early, because the
    // dispatcher did not exist yet. The dispatcher's own orphan re-dispatch is
    // the durable floor if an emit is lost anyway.
    //
    // Crash-interrupted threads were NOT queued, so this never re-runs work
    // that may have crashed the engine. The returned ids are what
    // `settle_unresumed_switch_threads` must NOT touch, so they are held until
    // that floor runs at the end of boot.
    let mut resumed_switch_threads: std::collections::HashSet<uuid::Uuid> = shared_engine
        .resume_pending_switches()
        .await
        .into_iter()
        .collect();

    // Scans agent_sessions from outside any per-thread `select!`, so a wedged
    // event handler cannot starve it the way it starves the in-loop watchdog.
    // Emits ContinuationRequested rather than ResponseAborted, so the
    // dispatcher above auto-resumes with no user-visible "Aborted" terminal.
    // See `engine::agent_session::external_watchdog`.
    let _watchdog_handle = shared_engine.spawn_external_watchdog();

    // Rebuild the Thread Queue's in-memory state BEFORE the scheduler starts.
    // The scheduler is the first submission path to come alive, so loading the
    // persisted backlog first keeps per-trigger FIFO intact across a restart.
    // An admitted row whose thread already materialized hands off to the
    // recovery above; one that bound no thread ran nothing, so it re-queues.
    shared_engine.thread_queue.recover_persisted_entries().await;

    // Keep the in-memory user-initiated pool a faithful mirror of each thread's
    // `thread_summaries.status` (ADR 0010). One subscriber reconciles the slot
    // on every status change.
    //
    // Without it the slot frees only when the chat task returns, which never
    // happens while a coding-agent thread is parked on a question. Subscribe
    // before the API server, whose chat handler acquires user slots.
    shared_engine.thread_queue.spawn_settle_subscriber();

    // Chat parity of `resume_pending_switches` above: auto-resume the chat /
    // trigger threads a user-initiated Switch to new version interrupted. Same
    // cause gate as the coding-agent half (a device-attributed EngineShutdown
    // teardown abort), so a crash still falls back to the manual Continue.
    //
    // Deliberately LATER than the coding-agent drain. A coding-agent resume
    // only needs the spawn dispatcher subscribed. A chat resume re-enters the
    // agentic loop directly and reads as `running` at once, so it must follow
    // `spawn_settle_subscriber()` above. Otherwise that status change
    // reconciles no Thread Queue slot. It must also follow
    // `recover_orphan_tool_calls`, or the re-entered turn rebuilds an unpaired
    // `tool_use` block.
    resumed_switch_threads.extend(shared_engine.resume_pending_chat_switches().await);

    // Boot floor (ADR 0045). Every *Switch to new version* promises the
    // interrupted thread a resume, and the UI withholds its Continue button on
    // the strength of that promise. Both drains above have had their turn, so
    // anything still holding an unkept promise has it WITHDRAWN here, which
    // hands the Continue affordance back. Runs before the API server accepts
    // traffic, so the user never sees the gap.
    shared_engine
        .settle_unresumed_switch_threads(&resumed_switch_threads)
        .await;

    let pool = shared_engine.pool().clone();

    let scheduler_engine = shared_engine.clone();
    let mut scheduler = SchedulerManager::new(scheduler_engine, pool.clone()).await?;
    scheduler.start().await?;
    let scheduler = Arc::new(tokio::sync::Mutex::new(scheduler));

    // Only now: draining consults trigger pause and deletion state, which the
    // scheduler's event replay just loaded.
    shared_engine.thread_queue.start_draining();

    // Resolved BEFORE the router is built, because it decides whether the
    // router carries a door (`api::local_auth`). The port it will be paired
    // with is read further down, where the listener is set up.
    let bind_choice = resolve_bind_choice(&shared_engine).await;

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
        &bind_choice,
    );
    // Dev-only: advance this engine's served-frontend snapshot to the
    // checkout-shared `dist/` when that is safe. A PEER workspace then picks up
    // another workspace's frontend-only Apply without a manual restart.
    // Spawned after `create_router`, so the served handle is registered before
    // the first tick. See
    // `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`.
    let _served_frontend_sync = shared_engine.spawn_served_frontend_sync();
    // Dev-only: reap a frontend preview a SIGKILLed predecessor left running,
    // since that child outlives the engine that spawned it. Then watch for its
    // worktree being reclaimed. See `engine::frontend_preview`.
    shared_engine.init_frontend_preview();
    // Keep `/api/v1/health`'s `database_reachable` honest. An engine outlives
    // its database, and without this the endpoint keeps reporting a healthy
    // engine while every other request fails. Not dev-only: a packaged
    // install's bundled Postgres can die too. See ADR 0037.
    let _db_health_probe = shared_engine.spawn_db_health_probe();
    // One-shot, idempotent: rebuild `ContextCaptured` rows for the auxiliary
    // model calls the engine made before it recorded them. Spawned rather
    // than awaited, unlike the backfills above. Nothing reads these rows
    // synchronously, so boot must not grow with the size of the events table.
    // See `core::aux_context_backfill`.
    let aux_backfill = shared_engine.clone();
    tokio::spawn(async move {
        match lucidos_engine::core::aux_context_backfill::backfill_auxiliary_captures(
            aux_backfill.pool(),
            &aux_backfill.event_bus,
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => log!(
                "[AuxContextBackfill] reconstructed {} auxiliary model call(s)",
                n
            ),
            Err(e) => log!("[AuxContextBackfill] failed: {}", e),
        }
    });
    // An unparseable value says so before falling back, the way a malformed
    // `LUCIDOS_BIND_ADDR` does. Silently binding 3000 sends the user hunting a
    // "connection refused" on the port they set. Blank counts as unset, as in
    // `api::base_path::api_port`: a launcher exporting an empty shell variable
    // asserted nothing.
    let api_port_env = std::env::var("LUCIDOS_API_PORT").ok();
    let api_port_raw = api_port_env
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let api_port = match api_port_raw {
        Some(raw) => raw.parse::<u16>().unwrap_or_else(|e| {
            log!("[Startup] LUCIDOS_API_PORT={raw:?} is not a port ({e}); using 3000");
            3000
        }),
        None => 3000,
    };
    // Every address to listen on. A specific `Address` ALSO binds loopback, so
    // the gateway probe, the dev scripts and the engine's own restart callback
    // keep working over `127.0.0.1`. `addr` is the primary, used only for the
    // startup log, and `bind_label` already notes the retained loopback.
    let addrs = net_config::bind_socket_addrs(&bind_choice, api_port);
    let addr = addrs[0];
    let bind_label = net_config::bind_scope_label(&bind_choice);

    let handle = axum_server::Handle::new();
    tokio::spawn(shutdown_signal(
        handle.clone(),
        shared_engine.clone(),
        scheduler,
    ));

    // The http/https decision is resolved in ONE place,
    // `net_config::tls_scheme_from`. The branch below only loads the cert paths
    // that decision implies.
    //
    // Trimmed at the read, because `tls_scheme_from` trims before deciding and
    // `from_pem_file` does not. A padded path would otherwise pick https and
    // then abort the boot on a file the user does have.
    let tls_cert = std::env::var("LUCIDOS_TLS_CERT")
        .ok()
        .map(|v| v.trim().to_string());
    let tls_key = std::env::var("LUCIDOS_TLS_KEY")
        .ok()
        .map(|v| v.trim().to_string());
    let scheme = net_config::tls_scheme_from(tls_cert.as_deref(), tls_key.as_deref());

    // Serve every resolved address concurrently, sharing the one graceful-shutdown
    // `Handle` so a single shutdown stops all sockets. A bind failure on any
    // address fails fast (same as the prior single-bind semantics).
    if scheme == net_config::SCHEME_HTTPS {
        // An https scheme means both paths are present and non-empty.
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

    /// Recurse with a large live stack frame per level, emulating the engine's
    /// deep un-spawned async poll chain. `#[inline(never)]`, `black_box` and
    /// reading `buf` AFTER the recursive call together defeat tail-call and
    /// dead-store optimization, so every level genuinely consumes stack.
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

    /// The engine's runtime must give its bounded-but-deep poll chain real
    /// headroom. See [`super::WORKER_THREAD_STACK_SIZE`].
    #[test]
    fn worker_stack_holds_deep_nested_async_chain() {
        // Must comfortably exceed tokio's 2 MiB default. Checked at compile
        // time, because the invariant is over a const.
        const {
            assert!(
                WORKER_THREAD_STACK_SIZE >= 8 * 1024 * 1024,
                "worker stack must exceed tokio's 2 MiB default with headroom"
            );
        }

        // A chain needing ~4 MiB of stack, on a worker thread of the
        // *production* runtime builder. On a 2 MiB stack this aborts the
        // process; on the configured stack it completes cleanly.
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
