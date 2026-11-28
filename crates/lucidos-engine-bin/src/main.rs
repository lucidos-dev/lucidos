#![deny(clippy::print_stdout, clippy::print_stderr)]

use lucidos_agents_api::api::create_router;
use lucidos_engine::engine::thread_lifecycle::ThreadStatus;
use lucidos_engine::engine::SharedEngine;
use lucidos_engine::engine_runtime::build_runtime;
use lucidos_engine::log_legacy;
use lucidos_engine::net_config;
use lucidos_engine::scheduler::SchedulerManager;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;

mod tcp_boot;

const TEARDOWN_DEADLINE: Duration = Duration::from_secs(20);

fn arm_teardown_deadline(deadline: Duration, on_expiry: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        log_legacy!(
            "[Shutdown] Teardown still running after {}s; exiting so a successor can own the workspace",
            deadline.as_secs()
        );
        on_expiry();
    });
}

const HTTP_DRAIN: Duration = Duration::from_secs(10);

async fn shutdown_signal(
    handles: Vec<axum_server::Handle>,
    socket_drain: tokio::sync::watch::Sender<bool>,
    engine: SharedEngine,
    scheduler: Arc<tokio::sync::Mutex<SchedulerManager>>,
) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let usr1 = async {
        signal::unix::signal(signal::unix::SignalKind::user_defined1())
            .expect("failed to install SIGUSR1 handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let usr1 = std::future::pending::<()>();

    #[cfg(unix)]
    let _sigterm_ignorer = tokio::spawn(async {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM ignorer");
        loop {
            sigterm.recv().await;
            log_legacy!(
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

    log_legacy!("\n[Shutdown] Shutting down gracefully...");
    arm_teardown_deadline(TEARDOWN_DEADLINE, || std::process::exit(0));

    let teardown_actor = engine.begin_teardown();

    let main_check_stopped = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.stop_main_check().await })
    };

    engine.abort_in_flight_for_restart(teardown_actor).await;

    engine.settle_running_background_tasks_at_teardown().await;
    engine
        .terminals
        .teardown(lucidos_engine::engine::bash_background_recovery::REAP_WAIT)
        .await;

    engine.shutdown_agent_sessions().await;
    engine.shutdown_active_threads().await;

    for handle in handles {
        handle.graceful_shutdown(Some(HTTP_DRAIN));
    }
    socket_drain.send_replace(true);

    engine.shutdown_browser().await;

    engine.stop_frontend_preview().await;

    shutdown_mcp_servers(&engine).await;

    if let Err(e) = main_check_stopped.await {
        log_legacy!("[Shutdown] Stopping the main check panicked: {}", e);
    }

    if let Err(e) = scheduler.lock().await.shutdown().await {
        log_legacy!("[Shutdown] Error shutting down scheduler: {}", e);
    }

    log_legacy!("[Shutdown] Shutdown complete.");
}

async fn shutdown_mcp_servers(engine: &SharedEngine) {
    let running: Vec<String> = match engine.mcp_manager.list_servers().await {
        Ok(servers) => servers
            .into_iter()
            .filter(|s| s.running)
            .map(|s| s.id)
            .collect(),
        Err(e) => {
            log_legacy!(
                "[Shutdown] Could not read the MCP servers to stop them: {}",
                e
            );
            return;
        }
    };
    if running.is_empty() {
        return;
    }
    log_legacy!("[Shutdown] Stopping {} MCP server(s)...", running.len());
    futures::future::join_all(running.iter().map(|id| async move {
        if let Err(e) = engine.mcp_manager.stop_server(id).await {
            log_legacy!("[Shutdown] Error stopping MCP server '{}': {}", id, e);
        }
    }))
    .await;
}

#[allow(clippy::print_stdout)]
fn metadata_probe() -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        println!(
            "Lucidos {} (lucidos-engine {})",
            lucidos_engine::build_identity().release,
            env!("CARGO_PKG_VERSION"),
        );
        return Ok(true);
    }

    if std::env::args().skip(1).any(|a| a == "--build-id") {
        if std::env::args().skip(1).any(|a| a == "--source-state") {
            let state = build_runtime()?
                .block_on(lucidos_engine::engine::engine_version::own_source_state());
            println!("{state}");
        } else {
            println!("{}", lucidos_engine::build_identity().id);
        }
        return Ok(true);
    }

    Ok(false)
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    lucidos_engine::BuildIdentity {
        id: env!("ENGINE_BUILD_ID"),
        release: include_str!("../../../RELEASE").trim_ascii_end(),
        release_dirty: env!("LUCIDOS_RELEASE_DIRTY") == "true",
        engine_version: include_str!("../../lucidos-engine/VERSION").trim(),
    }
    .install();
    if metadata_probe()? {
        return Ok(());
    }

    if std::env::args().skip(1).any(|a| a == "--wasm-selftest") {
        return match build_runtime()?.block_on(lucidos_engine::jit_selftest()) {
            Ok(()) => {
                log_legacy!("[Selftest] wasm JIT selftest passed");
                Ok(())
            }
            Err(e) => Err(format!("wasm JIT selftest failed: {e}").into()),
        };
    }

    if std::env::args().skip(1).any(|a| a == "--gateway") {
        return Err("`--gateway` was removed from lucidos-engine; run the \
                    `lucidos-gateway` binary instead (ADR 0014)"
            .into());
    }

    if std::env::args().nth(1).as_deref() == Some("restore-archive") {
        return build_runtime()?.block_on(lucidos_engine_boot::restore_archive::run());
    }

    {
        use lucidos_engine_boot::snapshot::{Arguments, Command};

        let command = match std::env::args().nth(1).as_deref() {
            Some("snapshot-restore") => Some(Command::Restore),
            Some("snapshot-recover") => Some(Command::Recover),
            _ => None,
        };
        if let Some(command) = command {
            let args = Arguments::parse(std::env::args().skip(2))?;
            let runtime = build_runtime()?;
            let database_url = lucidos_engine::core::database_url();
            return runtime.block_on(command.run(args, database_url));
        }
    }

    if std::env::args().nth(1).as_deref() == Some("secrets") {
        return build_runtime()?.block_on(lucidos_engine_boot::secrets::run());
    }

    if std::env::args().nth(1).as_deref() == Some("preflight") {
        return build_runtime()?.block_on(run_preflight());
    }

    lucidos_agents_api::api::gateway_stamp::take_stamp_key_from_starter();
    build_runtime()?.block_on(run())
}

async fn run_preflight() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use lucidos_engine::core::secret_store::key_store;

    let _ = dotenvy::dotenv();
    let workspace = std::env::var("LUCIDOS_WORKSPACE")
        .map_err(|_| "preflight: LUCIDOS_WORKSPACE env var is required")?;
    let scratch = PathBuf::from(workspace).join(".lucidos");
    std::fs::create_dir_all(&scratch)?;
    let store = key_store::resolve()?;
    lucidos_engine_boot::preflight::run(
        &lucidos_engine::core::database_url(),
        &scratch,
        store.as_ref(),
    )
    .await?;
    log_legacy!("[Preflight] This binary boots on a copy of the workspace database");
    Ok(())
}

/// Log once that a stored per-workspace bind is ignored. Every engine binds
/// loopback, and the gateway bind is the only network setting (ADR 0826).
async fn log_ignored_network_bind(engine: &SharedEngine) {
    use lucidos_engine::core::{preferences::PreferenceStore, prefs};
    match PreferenceStore::get_all(engine.pool()).await {
        Ok(stored) => match stored.get(prefs::NETWORK_BIND_RETIRED).map(|v| v.trim()) {
            Some(value) if !value.is_empty() && !value.eq_ignore_ascii_case("loopback") => {
                log_legacy!(
                    "[Startup] This workspace stores network_bind = {value:?}, which is ignored: \
                     engines bind loopback only. Set the gateway bind in the workspace picker's \
                     Network access control to reach Lucidos from other devices (ADR 0826)."
                )
            }
            _ => {}
        },
        Err(e) => log_legacy!("[Startup] could not read stored preferences ({e})"),
    }
}

async fn own_workspace(
    workspace: &std::path::Path,
) -> std::io::Result<lucidos_owner_lock::OwnerLock> {
    use lucidos_owner_lock::{engine, OwnerLock};
    let workspace = workspace.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let successor = OwnerLock::acquire(&engine::successor_lock_path(&workspace))?;
        successor.publish_pid(&engine::successor_pid_path(&workspace))?;
        let lock_path = engine::lock_path(&workspace);
        let lock = match OwnerLock::try_acquire(&lock_path)? {
            Some(lock) => lock,
            None => {
                log_legacy!(
                    "[Startup] Another engine still owns this workspace; waiting for it to exit"
                );
                OwnerLock::acquire(&lock_path)?
            }
        };
        lock.publish_pid(&engine::pid_path(&workspace))?;
        drop(successor);
        log_legacy!("[Startup] Acquired the workspace's owner lock");
        Ok(lock)
    })
    .await
    .map_err(std::io::Error::other)?
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let launched_as = lucidos_engine::runtime::lucidos_cli::launch_file_id();
    lucidos_engine::net_config::install_crypto_provider();

    lucidos_engine::core::load_database_environment();

    lucidos_engine_boot::host_setup::prepare();

    let workspace_path = std::env::var("LUCIDOS_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            if PathBuf::from("/workspace").exists() {
                PathBuf::from("/workspace")
            } else {
                PathBuf::from("./workspace")
            }
        });

    lucidos_engine::internal_faults::install(&workspace_path);

    lucidos_engine::core::snapshot::boot_guard::ensure_boot_entry(
        &lucidos_engine::core::database_url(),
    )?;
    lucidos_home::ensure_storage_root(&workspace_path)?;
    log_legacy!("[Startup] Using workspace: {}", workspace_path.display());
    if let Some(static_dir) = std::env::var_os("LUCIDOS_STATIC_DIR") {
        lucidos_engine::engine::register_as_build_watch_consumer(std::path::Path::new(&static_dir));
    }
    let _owner_lock = own_workspace(&workspace_path).await?;

    let api_port_env = std::env::var("LUCIDOS_API_PORT").ok();
    let api_port_raw = api_port_env
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let api_port = match api_port_raw {
        Some(raw) => raw.parse::<u16>().unwrap_or_else(|e| {
            log_legacy!("[Startup] LUCIDOS_API_PORT={raw:?} is not a port ({e}); using 3000");
            3000
        }),
        None => 3000,
    };
    let tls_cert = std::env::var("LUCIDOS_TLS_CERT")
        .ok()
        .map(|v| v.trim().to_string());
    let tls_key = std::env::var("LUCIDOS_TLS_KEY")
        .ok()
        .map(|v| v.trim().to_string());
    let scheme = net_config::tls_scheme_from(tls_cert.as_deref(), tls_key.as_deref());
    let tls_config = if scheme == net_config::SCHEME_HTTPS {
        let cert = tls_cert.unwrap_or_default();
        let key = tls_key.unwrap_or_default();
        Some(axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?)
    } else {
        None
    };
    let bound_tcp = lucidos_loopback_listener::bind(&[std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        api_port,
    ))])
    .inspect_err(|e| log_legacy!("[Startup] cannot bind port {api_port}: {e}"))?;
    if let Some(e) = &bound_tcp.ipv6_loopback_unavailable {
        let port = bound_tcp.port().unwrap_or(api_port);
        log_legacy!(
            "[Startup] {}",
            lucidos_loopback_listener::describe_unavailable(port, e)
        );
    }
    let mut tcp = tcp_boot::BootServer::start(bound_tcp.listeners, tls_config).await?;

    let boot_database_url = lucidos_engine::core::database_url();
    let restore_boot =
        if lucidos_engine::core::snapshot::boot_guard::ensure_normal_boot(&boot_database_url)
            .is_ok()
        {
            None
        } else {
            let guard = lucidos_engine::core::snapshot::first_boot::RestoreBoot::acquire(
                &_owner_lock,
                &workspace_path,
                &boot_database_url,
            )?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&boot_database_url)
                .await?;
            let verified = guard.verify(pool.clone()).await?;
            pool.close().await;
            Some(verified)
        };
    lucidos_engine::runtime::lucidos_cli::pin_cli_for_workspace(&workspace_path, launched_as);
    let socket_path = lucidos_engine_socket::path_for(&workspace_path).inspect_err(|e| {
        log_legacy!("[Startup] this workspace has no engine socket: {e}");
    })?;
    if let Some(name) = workspace_path.file_name().and_then(|n| n.to_str()) {
        lucidos_engine::core::keep_awake::set_workspace(name);
    }

    lucidos_engine::memory::apply_default_cache_dir(&workspace_path);

    lucidos_engine::llm::model_prices::read_overrides_from(workspace_path.clone());

    match lucidos_agents_api::api::proxy_migration::migrate_apis_json_if_needed(&workspace_path) {
        Ok(lucidos_agents_api::api::proxy_migration::MigrationOutcome::Migrated {
            backup_path,
        }) => {
            log_legacy!(
                "[Startup] migrated apis.json to pipeline shape (backup at {})",
                backup_path.display()
            );
        }
        Ok(_) => {}
        Err(e) => log_legacy!("[Startup] apis.json migration skipped: {}", e),
    }

    let proxy_load = lucidos_agents_api::api::load_proxy_config(&workspace_path);
    let seeded_handshake_scripts = match lucidos_engine::core::handshake_approvals::seed_if_absent(
        &workspace_path,
        &proxy_load.handshake_seed_entries(),
    ) {
        Ok(seeded) => seeded,
        Err(e) => {
            log_legacy!("[Startup] could not seed handshake approvals: {}", e);
            Vec::new()
        }
    };
    for path in &seeded_handshake_scripts {
        log_legacy!(
            "[Startup] approved the handshake script already in use: {}",
            path
        );
    }
    let configured_credential_scopes = proxy_load.credential_scopes();
    let proxy_config_rejections = proxy_load.rejected;
    for rejected in &proxy_config_rejections {
        log_legacy!(
            "[Startup] apis.json: refusing '{}': {}",
            rejected.label(),
            rejected.reason
        );
    }
    let proxy_insecure_transports = proxy_load.insecure;
    for insecure in &proxy_insecure_transports {
        log_legacy!(
            "[Startup] apis.json: '{}' has an unauthenticated transport: {}",
            insecure.provider,
            insecure.reason
        );
    }

    let database_url = lucidos_engine::core::database_url();
    log_legacy!("[Startup] Connecting to PostgreSQL...");
    let mut boot_stages = lucidos_engine::boot_report::BootStageTimer::start();

    let provider_ctx =
        lucidos_engine_boot::provider_context::build_provider_context(&database_url).await;

    let agent_host = Arc::new(lucidos_agents_api::engine::agent_recovery::EngineAgentHost);
    let workspace_providers =
        Box::new(lucidos_engine::engine::provider_adapters::ProviderAdapters::default());
    let engine = lucidos_engine_boot::construction::construct_engine(
        workspace_path.clone(),
        &database_url,
        provider_ctx,
        agent_host,
        workspace_providers,
        restore_boot,
    )
    .await?;
    lucidos_engine::internal_faults::start(&workspace_path, engine.event_bus.clone());
    log_legacy!("[Startup] PostgreSQL connected");
    boot_stages.lap("engine setup");

    lucidos_engine::core::environment_variables::migrate_env_file_to_db(
        engine.pool(),
        &engine.event_bus,
        &workspace_path,
    )
    .await;
    lucidos_engine::core::PreferenceStore::move_legacy_chat_effort(
        engine.pool(),
        &engine.event_bus,
        engine.current_provider().default_model(),
    )
    .await;
    lucidos_engine::core::PreferenceStore::replace_retired_values(engine.pool(), &engine.event_bus)
        .await;
    engine.upgrade_chat_ladder_from_enabled().await;
    lucidos_engine::core::environment_variables::apply_to_process_env(engine.pool()).await;
    if let Err(e) = lucidos_engine::diagnostic_log::start(&workspace_path) {
        log_legacy!("[Startup] diagnostic log not started: {e}");
    }

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
                        audience: None,
                        push_group: None,
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
                        audience: None,
                        push_group: None,
                    },
                ),
                "[Proxy] insecure transport NotificationCreated",
            )
            .await;
    }

    match lucidos_engine::core::CredentialStore::infer_undeclared_scopes(
        engine.pool(),
        &engine.event_bus,
        &configured_credential_scopes,
    )
    .await
    {
        Ok(scoped) => {
            for (name, base_url) in scoped {
                log_legacy!("[Startup] scoped credential '{}' to {}", name, base_url);
            }
        }
        Err(e) => log_legacy!("[Startup] could not infer credential scopes: {}", e),
    }

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
                        audience: None,
                        push_group: None,
                    },
                ),
                "[Proxy] handshake seed NotificationCreated",
            )
            .await;
    }

    let event_store = engine.event_store().clone();
    let embedder = engine.embedder().clone();
    let memory_index = engine.memory_index().clone();

    let shared_engine: SharedEngine = Arc::new(engine);
    shared_engine.set_self_arc(&shared_engine);

    shared_engine.session_spawn_gate.hold();
    let serving = shared_engine.session_spawn_gate.subscribe();
    let event_hooks = Arc::new(lucidos_event_hooks::WorkspaceHooks);
    let _ = shared_engine.event_hook_catalog.set(event_hooks.clone());
    let asker = Arc::new(lucidos_event_hooks::WorkspaceHookAsker::new(
        &shared_engine,
        serving.clone(),
    ));
    if let Err(e) = shared_engine
        .event_bus
        .install_event_hook_asker(asker)
        .await
    {
        log_legacy!("[Boot] The armed event hooks were not preloaded: {e}");
    }
    lucidos_engine::engine::user_only_message_delivery::start(&shared_engine);
    shared_engine.event_bus.hold_parent_wakes();
    shared_engine.start_parent_callback_listener();
    shared_engine.start_apply_all_driver();
    shared_engine.start_landing_lane_driver();
    lucidos_engine::landing_gateway::enable();
    lucidos_engine::gateway_users::enable();
    lucidos_engine::gateway_devices::enable(shared_engine.pool.clone());
    lucidos_engine_boot::follow_main::spawn(&shared_engine);
    shared_engine.start_main_check_driver();
    shared_engine.start_standing_apply_resolver();
    lucidos_agents_api::engine::agent_recovery::start_archive_request_resolver(&shared_engine);
    shared_engine.spawn_embedder_load();
    lucidos_engine::boot_report::report(lucidos_engine::boot_report::RECOVERING);
    boot_stages.lap("pre-recovery setup");

    lucidos_engine::engine::supervisor_respawn_sidecar::emit_if_present(
        &workspace_path,
        &shared_engine.event_bus,
    )
    .await;

    lucidos_engine::engine::permission::tool_grants::import_legacy_files(
        &shared_engine.event_bus,
        &shared_engine.grants_dir(),
    )
    .await;
    lucidos_agents_api::engine::agent_recovery::recover_orphan_cc_permission_requests(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;
    lucidos_engine::engine::command_permission::recover_orphan_command_permission_requests(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;
    lucidos_engine::engine::mcp_permission::recover_orphan_mcp_permission_requests(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;
    lucidos_agents_api::engine::agent_recovery::recover_unsettled_side_questions(&shared_engine)
        .await;
    lucidos_engine::engine::form_requests::expire_memory_backed_requests(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;
    lucidos_engine::voice::recovery::settle_orphan_voice_sessions(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;
    boot_stages.lap("orphan requests");

    if let Err(e) = sqlx::query(&format!(
        "UPDATE thread_summaries SET status = {}, coding_agent_is_external_repo = FALSE \
         WHERE status = {} AND coding_agent_change_state <> '{}' AND source = 'claude_code'",
        ThreadStatus::Idle.sql_literal(),
        ThreadStatus::Waiting.sql_literal(),
        lucidos_engine::engine::thread_lifecycle::ChangeStateKind::Proposed.as_str(),
    ))
    .execute(shared_engine.pool())
    .await
    {
        log_legacy!("[Startup] Failed to reset orphaned waiting threads: {}", e);
    }

    if let Err(e) = lucidos_engine::engine::home_thread::ensure_home_thread(
        &shared_engine.event_bus,
        shared_engine.pool(),
    )
    .await
    {
        log_legacy!("[Startup] Failed to create the home thread: {}", e);
    }

    lucidos_engine::engine::todo_consumer::spawn(shared_engine.clone());
    boot_stages.lap("summary resets");

    lucidos_engine::runtime::claude_code::sweep_stale_session_files(shared_engine.workspace_path());

    let recovering_threads =
        lucidos_agents_api::engine::agent_recovery::recover_orphaned_worktrees(&shared_engine)
            .await;
    boot_stages.lap("worktree recovery");

    lucidos_agents_api::engine::agent_recovery::recover_orphan_tool_calls(&shared_engine).await;
    boot_stages.lap("orphan tool calls");

    lucidos_agents_api::engine::agent_recovery::recover_orphaned_threads(
        &shared_engine,
        &recovering_threads,
    )
    .await;
    boot_stages.lap("chat recovery");

    lucidos_agents_api::engine::agent_recovery::settle_orphaned_running_threads(
        shared_engine.pool(),
        &shared_engine.event_bus,
        &recovering_threads.iter().copied().collect(),
    )
    .await;

    lucidos_agents_api::engine::agent_recovery::settle_stranded_trigger_runs(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;

    if let Err(e) =
        lucidos_engine::engine::event_bus::EventBus::rebuild_children_counts(shared_engine.pool())
            .await
    {
        log_legacy!("[Startup] Failed to reconcile the child counts: {}", e);
    }

    if let Err(e) = lucidos_engine::engine::event_bus::EventBus::rebuild_blocking_descendant_count(
        shared_engine.pool(),
    )
    .await
    {
        log_legacy!(
            "[Startup] Failed to reconcile blocking_descendant_count: {}",
            e
        );
    }

    lucidos_agents_api::engine::agent_recovery::withdraw_incomplete_pending_changes_on_startup(
        shared_engine.pool(),
        &shared_engine.event_bus,
    )
    .await;

    lucidos_agents_api::engine::agent_recovery::refresh_branch_work_on_startup(
        shared_engine.pool(),
        shared_engine.workspace_path(),
    )
    .await;
    boot_stages.lap("has-diff sweep");

    let held_back_sweep_waits = !matches!(
        shared_engine
            .event_bus
            .hooks_armed_on("ChangeProposed")
            .await,
        Ok(false)
    );
    let held_back_sweep_waits = held_back_sweep_waits
        || !matches!(
            shared_engine
                .event_bus
                .hooks_armed_on(lucidos_engine::core::event_hooks::PROPOSAL_CHECK_EVENT,)
                .await,
            Ok(false)
        );
    if !held_back_sweep_waits {
        lucidos_agents_api::engine::agent_recovery::propose_held_back_changes_on_startup(
            shared_engine.pool(),
            &shared_engine.event_bus,
            shared_engine.workspace_path(),
        )
        .await;
    }

    lucidos_agents_api::engine::agent_recovery::reconcile_emptied_changes_on_startup(
        &shared_engine,
    )
    .await;
    boot_stages.lap("change reconcile");

    lucidos_engine::engine::permission::batch::close_cut_off_batches_on_startup(
        &shared_engine.event_bus,
    )
    .await;

    {
        let engine = shared_engine.clone();
        tokio::spawn(async move {
            lucidos_agents_api::engine::agent_recovery::set_aside_archived_branch_work_on_startup(
                &engine,
            )
            .await
        });
    }

    lucidos_engine::engine::thread_hold::hold_set_aside_changes(&shared_engine.event_bus, None)
        .await;

    shared_engine
        .event_bus
        .refire_unprocessed_child_completions()
        .await;

    shared_engine.event_bus.reset_keep_alive_turns().await;
    shared_engine.start_wait_reentry_consumer();
    shared_engine.start_event_wait_dispatcher();
    lucidos_agents_api::engine::agent_recovery::start_background_task_reaper(&shared_engine);
    lucidos_engine::engine::terminal::lifecycle::recover_at_boot(&shared_engine).await;
    lucidos_engine::engine::terminal::lifecycle::start(&shared_engine);
    shared_engine.refire_unresolved_wait_reentries().await;

    shared_engine.settle_abandoned_background_tasks().await;

    shared_engine.rebuild_event_waits().await;
    boot_stages.lap("event waits");

    let released = shared_engine.event_bus.release_held_parent_wakes();
    if released > 0 {
        log_legacy!("[Startup] Released {released} parent wake(s) held during recovery");
    }

    lucidos_agents_api::engine::agent_recovery::begin_allowed_starts_on_startup(&shared_engine)
        .await;

    shared_engine.recover_apply_all_batches().await;

    shared_engine.recover_landing_lane().await;

    shared_engine.recover_main_check();

    shared_engine.recover_standing_applies().await;
    boot_stages.lap("apply recovery");

    lucidos_engine::engine::memory_consumer::spawn(shared_engine.clone());
    lucidos_engine_boot::config_change_notifications::spawn(
        shared_engine.event_bus.clone(),
        shared_engine.pool().clone(),
    );
    lucidos_engine::engine::change_summary_consumer::spawn(shared_engine.clone());
    lucidos_engine::engine::pending_change_watch::spawn(shared_engine.clone());
    lucidos_engine::engine::card_queue::spawn(shared_engine.clone());
    lucidos_engine::engine::permission::words::spawn(shared_engine.clone());
    lucidos_engine::engine::summary_tree::spawn(shared_engine.clone());
    lucidos_agents_api::engine::agent_recovery::spawn_branch_work_net(shared_engine.clone());
    lucidos_agents_api::engine::successor::start_pending_successor_sessions(shared_engine.clone())
        .await;

    lucidos_agents_api::engine::agent_recovery::start_cc_model_discovery(&shared_engine).await;

    let cleanup_wake = std::sync::Arc::new(tokio::sync::Notify::new());
    lucidos_engine::engine::worktree_cleanup::WorktreeCleanup::spawn(
        shared_engine.pool().clone(),
        std::sync::Arc::new(shared_engine.event_bus.clone()),
        workspace_path.clone(),
        shared_engine.worktree_cleanup_active_threads(),
        cleanup_wake.clone(),
    );
    lucidos_engine::engine::worktree_cleanup::DiskMonitor::spawn(
        std::sync::Arc::new(shared_engine.event_bus.clone()),
        workspace_path.clone(),
        cleanup_wake,
    );

    let (_dispatcher_handle, spawn_request_rx) =
        lucidos_agents_api::engine::agent_recovery::SpawnDispatcher::spawn(
            shared_engine.pool().clone(),
            std::sync::Arc::new(shared_engine.event_bus.clone()),
        );
    shared_engine.start_spawn_request_consumer(spawn_request_rx);

    lucidos_agents_api::engine::agent_recovery::ArchivableCountWorker::spawn(
        shared_engine.pool().clone(),
        std::sync::Arc::new(shared_engine.event_bus.clone()),
    );

    let mut resumed_switch_threads: std::collections::HashSet<uuid::Uuid> = shared_engine
        .resume_pending_switches()
        .await
        .into_iter()
        .collect();

    let _watchdog_handle =
        lucidos_agents_api::engine::agent_recovery::spawn_external_watchdog(&shared_engine);

    shared_engine.thread_queue.begin_boot_recovery();
    shared_engine.thread_queue.recover_persisted_entries().await;

    shared_engine.thread_queue.spawn_drain_hint_subscriber();

    resumed_switch_threads.extend(
        lucidos_agents_api::engine::agent_recovery::resume_pending_chat_switches(&shared_engine)
            .await,
    );

    lucidos_agents_api::engine::agent_recovery::settle_agent_switch_threads(
        &shared_engine,
        &resumed_switch_threads,
    )
    .await;
    boot_stages.lap("switch resumes");

    let pool = shared_engine.pool().clone();

    let scheduler_engine = shared_engine.clone();
    let mut scheduler = SchedulerManager::new(scheduler_engine, pool.clone()).await?;
    scheduler.start().await?;
    let scheduler = Arc::new(tokio::sync::Mutex::new(scheduler));

    lucidos_engine_boot::release_notice_boot::seed_cursor_at_startup(
        shared_engine.pool(),
        &shared_engine.trigger_configs,
    )
    .await;

    shared_engine.thread_queue.start_draining();
    boot_stages.lap("scheduler");

    log_ignored_network_bind(&shared_engine).await;

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
        event_hooks,
    );
    shared_engine.record_engine_version_activated().await;
    let _served_frontend_sync =
        lucidos_engine_boot::frontend_tasks::spawn_served_frontend_sync(shared_engine.clone());
    let _version_status_watch =
        lucidos_engine_boot::version_watch::spawn_version_status_watch(shared_engine.clone());
    lucidos_engine_boot::frontend_tasks::init_frontend_preview(shared_engine.clone());
    let _db_health_probe = lucidos_engine_boot::db_health_probe::spawn(shared_engine.clone());
    let aux_backfill = shared_engine.clone();
    tokio::spawn(async move {
        match lucidos_engine::core::aux_context_backfill::backfill_auxiliary_captures(
            aux_backfill.pool(),
            &aux_backfill.event_bus,
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => log_legacy!(
                "[AuxContextBackfill] reconstructed {} auxiliary model call(s)",
                n
            ),
            Err(e) => log_legacy!("[AuxContextBackfill] failed: {}", e),
        }
    });

    if !lucidos_engine::gateway_users::await_first_answer().await {
        let engine = shared_engine.clone();
        tokio::spawn(async move {
            lucidos_agents_api::api::announce_users_if_changed(
                &engine.event_bus,
                lucidos_engine::gateway_users::late_first_answer(),
            )
            .await;
        });
    }

    if lucidos_engine::gateway_devices::known().is_none() {
        let engine = shared_engine.clone();
        tokio::spawn(async move {
            lucidos_agents_api::api::announce_renamed_devices(
                &engine.event_bus,
                lucidos_engine::gateway_devices::late_first_answer().await,
            )
            .await;
        });
    }

    let socket_listener = lucidos_engine_socket::bind(&socket_path).inspect_err(|e| {
        log_legacy!("[Startup] cannot bind the engine socket: {e}");
    })?;
    log_legacy!("[Startup] Engine socket at {}", socket_path.display());
    let (socket_drain, socket_draining) = tokio::sync::watch::channel(false);
    let socket_server = lucidos_agents_api::api::engine_socket::serve(
        socket_listener,
        socket_path.clone(),
        app.clone(),
        socket_draining,
        HTTP_DRAIN,
    );

    if held_back_sweep_waits {
        let engine = shared_engine.clone();
        let mut serving = serving.clone();
        tokio::spawn(async move {
            if serving.wait_for(|serving| *serving).await.is_ok() {
                lucidos_agents_api::engine::agent_recovery::propose_held_back_changes_on_startup(
                    engine.pool(),
                    &engine.event_bus,
                    engine.workspace_path(),
                )
                .await;
            }
        });
    }
    let shutdown = tokio::spawn(shutdown_signal(
        tcp.handles().to_vec(),
        socket_drain,
        shared_engine.clone(),
        scheduler,
    ));

    boot_stages.lap("router");
    log_legacy!("[Startup] Boot stages: {}", boot_stages.summary());

    tcp.publish(app);
    log_legacy!(
        "[Startup] API server listening on {}://{}  (loopback only)",
        scheme,
        tcp.address()
    );
    shared_engine.session_spawn_gate.open();
    serve_until_shutdown(tcp.join(), socket_server).await?;

    if let Err(e) = shutdown.await {
        log_legacy!("[Shutdown] teardown task failed: {e}");
    }
    Ok(())
}

async fn serve_until_shutdown<T, E>(
    tcp: impl std::future::Future<Output = Result<T, E>>,
    socket: impl std::future::Future<Output = ()>,
) -> Result<(), E> {
    tokio::pin!(tcp);
    tokio::pin!(socket);
    tokio::select! {
        served = &mut tcp => {
            served?;
            socket.await;
        }
        () = &mut socket => {
            tcp.await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod ownership_tests {
    use super::{arm_teardown_deadline, own_workspace};
    use std::time::Duration;

    #[test]
    fn a_teardown_that_never_finishes_still_expires() {
        let (tx, rx) = std::sync::mpsc::channel();
        arm_teardown_deadline(Duration::from_millis(1), move || tx.send(()).unwrap());
        rx.recv().expect("the deadline fired");
    }

    #[test]
    fn the_stop_signal_arms_the_teardown_deadline() {
        let source = include_str!("main.rs");
        let start = source
            .find("async fn shutdown_signal(")
            .expect("shutdown_signal exists");
        let body = &source[start..start + source[start..].find("\n}\n").expect("it ends")];
        let signalled = body.find("tokio::select!").expect("it waits for a signal");
        let armed = body
            .find("arm_teardown_deadline(TEARDOWN_DEADLINE")
            .expect("the stop path arms the teardown deadline");
        let teardown = body.find("begin_teardown()").expect("it tears down");
        assert!(
            signalled < armed && armed < teardown,
            "armed on the signal, before teardown"
        );
    }

    #[test]
    fn the_workspace_is_a_storage_root_before_it_is_owned() {
        let source = include_str!("main.rs");
        let body = &source[source.find("async fn run()").expect("run() exists")..];
        let asserted = body
            .find("ensure_storage_root(&workspace_path)")
            .expect("run() makes the workspace a storage root");
        let owned = body
            .find("own_workspace(&workspace_path)")
            .expect("run() takes the workspace's owner lock");
        assert!(
            asserted < owned,
            "the owner lock writes into the workspace first"
        );
    }

    #[test]
    fn the_owner_lock_comes_before_anything_stateful() {
        let source = include_str!("main.rs");
        let body = &source[source.find("async fn run()").expect("run() exists")..];
        let owned = body
            .find("own_workspace(&workspace_path)")
            .expect("run() takes the workspace's owner lock");
        for stateful in [
            "PgPoolOptions",
            "boot_report::report",
            "recover_orphaned",
            "lucidos_engine_socket::bind",
            "lucidos_loopback_listener::bind",
        ] {
            let at = body
                .find(stateful)
                .unwrap_or_else(|| panic!("{stateful} moved"));
            assert!(owned < at, "{stateful} runs before the owner lock is held");
        }
    }

    #[tokio::test]
    async fn a_successor_owns_the_workspace_only_after_its_predecessor() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = lucidos_owner_lock::engine::lock_path(dir.path());
        let pid_path = lucidos_owner_lock::engine::pid_path(dir.path());
        let predecessor = lucidos_owner_lock::OwnerLock::acquire(&lock_path).unwrap();
        std::fs::write(&pid_path, "4242").unwrap();

        let workspace = dir.path().to_path_buf();
        let successor = tokio::spawn(async move { own_workspace(&workspace).await });
        while lucidos_owner_lock::engine::waiting_pid(dir.path()).is_none() {
            tokio::task::yield_now().await;
        }
        assert!(
            !successor.is_finished(),
            "the successor ran while the predecessor lived"
        );
        assert_eq!(std::fs::read_to_string(&pid_path).unwrap(), "4242");
        assert_eq!(
            lucidos_owner_lock::engine::waiting_pid(dir.path()),
            Some(std::process::id())
        );

        drop(predecessor);
        let lock = successor.await.unwrap().expect("the lock frees");
        assert_eq!(
            lucidos_owner_lock::engine::live_pid(dir.path()),
            Some(std::process::id())
        );
        assert_eq!(lucidos_owner_lock::engine::waiting_pid(dir.path()), None);
        drop(lock);
    }
}

#[cfg(test)]
mod tcp_bind_tests {
    #[test]
    fn the_api_port_binds_through_the_loopback_pair() {
        let source = include_str!("main.rs");
        let tests = source
            .find("mod tcp_bind_tests")
            .expect("this module exists");
        let body = &source[..tests];
        let tcp_bind = body
            .find("lucidos_loopback_listener::bind(&[std::net::SocketAddr::from((")
            .expect("the API port binds through the loopback pair");
        let tcp_boot = body
            .find("tcp_boot::BootServer::start(")
            .expect("boot serves a startup response");
        assert!(tcp_bind < tcp_boot);
        for startup_step in [
            "RestoreBoot::acquire(",
            "construction::construct_engine(",
            "session_spawn_gate.hold()",
            "agent_recovery::start_archive_request_resolver(",
            "agent_recovery::recover_orphan_cc_permission_requests(",
        ] {
            let step = body.find(startup_step).expect("boot still runs this step");
            assert!(
                tcp_boot < step,
                "the API port must be listening before {startup_step}"
            );
        }
        let switch_recovery = body
            .find("agent_recovery::settle_agent_switch_threads(")
            .expect("boot recovers switch threads");
        let socket_bind = body
            .find("lucidos_engine_socket::bind(")
            .expect("boot binds the engine socket");
        let publish = body
            .find("tcp.publish(app)")
            .expect("boot publishes the live router");
        let resume = body
            .find("session_spawn_gate.open()")
            .expect("boot releases the session spawn gate");
        assert!(tcp_boot < switch_recovery);
        assert!(switch_recovery < socket_bind && socket_bind < publish && publish < resume);
        for late_bind in ["axum_server::bind(", "axum_server::bind_rustls("] {
            assert!(
                !body.contains(late_bind),
                "{late_bind} binds inside serve, so it would skip the [::1] half"
            );
        }
    }
}
