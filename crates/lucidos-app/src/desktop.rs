//! Always-on desktop runtime for a PACKAGED Lucidos build.
//!
//! The workspace gateway + bundled Postgres run as a persistent **macOS launchd
//! LaunchAgent** (`~/Library/LaunchAgents/com.lucidos.engine.plist`,
//! `RunAtLoad` + `KeepAlive`), independent of any window. Closing the UI does
//! NOT stop them — triggers, scheduled tasks, coding-agent sessions, and mobile
//! push keep running headless. The Tauri window and the mobile PWA are pure
//! clients of the service.
//!
//! Two roles share this one bundled binary:
//!
//!  * **Service** (`Lucidos --service`, started by launchd): [`run_service`]
//!    spawns + supervises the standalone **workspace gateway** (the
//!    `lucidos-gateway` binary, ADR 0014) on a STABLE port. The gateway owns the
//!    rest — it provisions the embedded Postgres + spawns one engine per
//!    registered workspace and reverse-proxies `/<slug>/` (first run auto-creates
//!    `default`). No window, no AppKit. On crash launchd respawns the service (the
//!    new gateway re-adopts already-running engines); on `launchctl bootout` (an
//!    explicit "Quit Lucidos") it tears the whole stack down and stays stopped.
//!  * **Client** (the GUI app the user double-clicks): [`launch`] ensures the
//!    service is installed + running, waits for `/~/api/v1/health` (the gateway),
//!    then points the window at it (the gateway serves the workspace picker behind
//!    the sigil namespace `/~/`). Window close leaves the service running.
//!
//! None of this runs in development — `scripts/tauri-dev.sh` keeps using Docker
//! Postgres + a natively-built engine, and [`launch`] short-circuits on
//! `tauri::is_dev()`.
//!
//! Bundle layout (Tauri `resources`, resolved at runtime relative to the
//! executable so the service — which has no `AppHandle` — resolves the same
//! paths the client's `resource_dir()` would):
//! ```text
//!   <resources>/postgres/bin/{initdb,pg_ctl,postgres,psql,createdb}  relocatable PG
//!   <resources>/postgres/lib, <resources>/postgres/share/...         libpq + pgvector
//!   <resources>/lucidos-gateway                                      workspace gateway binary
//!   <resources>/lucidos-engine                                       the engine binary
//!   <resources>/frontend/                                            the built UI (dist)
//!   <resources>/sdk/                                                 the built JS SDK
//! ```
//! State (the Postgres cluster + the workspace's `data/`) lives under the OS
//! app-data dir so it survives app updates — the updater replaces the `.app`,
//! never app-data.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Bundle identifier — must match `tauri.conf.json` `identifier`. Used to
/// resolve the OS app-data dir from the service role, which has no `AppHandle`.
const BUNDLE_IDENTIFIER: &str = "com.lucidos.app";

/// Historical launchd label for the always-on gateway service. The plist
/// installs at `~/Library/LaunchAgents/<LAUNCH_AGENT_LABEL>.plist`.
pub const LAUNCH_AGENT_LABEL: &str = "com.lucidos.engine";

/// Fixed default gateway port so the mobile connect URL is stable across
/// restarts. The historical `engine_port` name is kept for callers, but in ADR
/// 0014 packaged builds the gateway owns this public port and spawned engines
/// bind loopback-only per workspace. Configurable: override with
/// `LUCIDOS_ENGINE_PORT`, or edit `<app-data>/config/engine-port`.
pub const DEFAULT_ENGINE_PORT: u16 = 5252;

/// How long to wait for the gateway to answer `/~/api/v1/health`
/// (migrations + embedding-model warmup can be slow on a fresh workspace).
const ENGINE_HEALTH_TIMEOUT: Duration = Duration::from_secs(120);

const GATEWAY_RESOURCE_NAME: &str = "lucidos-gateway";
const ENGINE_RESOURCE_NAME: &str = "lucidos-engine";
const FRONTEND_RESOURCE_NAME: &str = "frontend";
const SDK_RESOURCE_NAME: &str = "sdk";
const POSTGRES_RESOURCE_NAME: &str = "postgres";

/// Set by the service's SIGTERM/SIGINT handler (launchd sends SIGTERM on
/// `bootout` / `kickstart -k`). The supervise loop observes it and tears the
/// embedded stack down gracefully. Atomic store is async-signal-safe.
static SERVICE_STOP: AtomicBool = AtomicBool::new(false);

/// The workspace gateway process the SERVICE role owns and supervises (ADR
/// 0014). The gateway (a standalone `lucidos-gateway` child) provisions the
/// embedded Postgres + spawns one engine per registered workspace itself, so the
/// service role no longer manages Postgres or per-workspace engines directly.
struct GatewayService {
    gateway: Child,
}

impl GatewayService {
    /// Stop the gateway, then the engines it spawned, then the embedded Postgres
    /// cluster. Best-effort; logs failures.
    ///
    /// This is the PERMANENT-stop teardown — "Quit Lucidos" (`bootout`) and the
    /// supervised-exit path both route here. It is NOT reached by the gateway's
    /// in-place reload (`execv`, same PID), which deliberately leaves the cluster
    /// running so the re-exec'd image re-adopts it; stopping Postgres here would
    /// break that re-adoption, so the stop lives in this path only.
    fn shutdown(&mut self, resources: &Path, app_data: &Path) {
        // The gateway ignores SIGTERM (to survive accidental `xargs kill` from CC
        // test scripts — see engine main.rs); SIGUSR1 is its graceful-stop
        // signal. It exits but deliberately LEAVES its engines running for
        // re-adoption, so we stop those explicitly below.
        #[cfg(unix)]
        {
            let pid = self.gateway.id().to_string();
            let _ = Command::new("kill").args(["-USR1", &pid]).status();
            for _ in 0..30 {
                match self.gateway.try_wait() {
                    Ok(Some(_)) => break,
                    _ => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
        let _ = self.gateway.kill();
        let _ = self.gateway.wait();
        stop_workspace_engines(app_data);
        // Stop the embedded cluster last — after the engines that connect to it —
        // so a permanent shutdown can never leave an orphaned `postgres` holding
        // the port + postmaster.pid for the next app version to trip over.
        stop_embedded_postgres(resources, app_data);
    }
}

/// Stop the embedded Postgres cluster cleanly (`pg_ctl -m fast`) on a permanent
/// service shutdown. Best-effort + logged; a no-op when no cluster has been
/// provisioned (no `<app-data>/pgdata/PG_VERSION`). Uses the bundled `pg_ctl` and
/// libpath — `lucidos-app` links neither the gateway nor engine crate, so this
/// shells out exactly as the rest of the service role does.
fn stop_embedded_postgres(resources: &Path, app_data: &Path) {
    let data = app_data.join("pgdata");
    if !data.join("PG_VERSION").exists() {
        return; // no embedded cluster to stop
    }
    let bundle = bundled_resources(resources);
    match embedded_pg_stop_command(&bundle.pg_bin, &bundle.pg_lib, &data).status() {
        Ok(s) if s.success() => eprintln!("[service] embedded Postgres stopped"),
        Ok(s) => eprintln!("[service] pg_ctl stop exited with {s}"),
        Err(e) => eprintln!("[service] failed to run pg_ctl stop: {e}"),
    }
}

/// Build the `pg_ctl -D <data> -m fast -w stop` command against the bundled
/// binaries, with the bundled libpath set (mirrors the gateway's
/// `with_pg_libpath`). `-m fast` disconnects any still-connected engine clients
/// and shuts the postmaster down cleanly; `-w` waits for it to finish.
fn embedded_pg_stop_command(pg_bin: &Path, pg_lib: &Path, data: &Path) -> Command {
    let mut cmd = Command::new(pg_bin.join("pg_ctl"));
    cmd.env("DYLD_LIBRARY_PATH", pg_lib);
    cmd.env("LD_LIBRARY_PATH", pg_lib);
    cmd.arg("-D").arg(data);
    cmd.args(["-m", "fast", "-w", "stop"]);
    cmd
}

/// SIGUSR1 every workspace engine the gateway spawned (pidfiles under
/// `<app-data>/workspaces/<id>/.lucidos/engine.pid`). Used on a full service stop
/// ("Quit Lucidos"); the gateway leaves them running on its own SIGUSR1 so they
/// can be re-adopted across a gateway restart, but an explicit stop tears the
/// whole stack down.
fn stop_workspace_engines(app_data: &Path) {
    let workspaces = app_data.join("workspaces");
    let Ok(entries) = std::fs::read_dir(&workspaces) else {
        return;
    };
    for entry in entries.flatten() {
        let pidfile = entry.path().join(".lucidos/engine.pid");
        let Ok(contents) = std::fs::read_to_string(&pidfile) else {
            continue;
        };
        if let Ok(pid) = contents.trim().parse::<i32>() {
            #[cfg(unix)]
            // SAFETY: SIGUSR1 to a recorded engine pid; a dead pid returns ESRCH.
            unsafe {
                libc::kill(pid, libc::SIGUSR1);
            }
            let _ = std::fs::remove_file(&pidfile);
        }
    }
}

// ── Path resolution (shared by both roles) ──────────────────────────────────

/// Resolve the bundle's `Resources` dir relative to the executable. In a
/// macOS `.app` the executable is `Contents/MacOS/Lucidos` and resources live
/// in `Contents/Resources/`, which is exactly what the client's
/// `resource_dir()` returns — so the service resolves the same bundle.
fn resource_dir_from_exe() -> io::Result<PathBuf> {
    resource_dir_for_exe(&std::env::current_exe()?)
}

fn resource_dir_for_exe(exe: &Path) -> io::Result<PathBuf> {
    // <Contents>/MacOS/Lucidos -> <Contents>/MacOS -> <Contents>
    let contents = exe
        .parent()
        .and_then(|macos| macos.parent())
        .ok_or_else(|| io::Error::other("cannot resolve bundle Contents dir"))?;
    Ok(contents.join("Resources"))
}

#[derive(Debug, Clone)]
struct BundledResources {
    gateway_bin: PathBuf,
    engine_bin: PathBuf,
    frontend: PathBuf,
    sdk: PathBuf,
    pg_bin: PathBuf,
    pg_lib: PathBuf,
}

fn bundled_resources(resources: &Path) -> BundledResources {
    let postgres = resources.join(POSTGRES_RESOURCE_NAME);
    BundledResources {
        gateway_bin: resources.join(GATEWAY_RESOURCE_NAME),
        engine_bin: resources.join(ENGINE_RESOURCE_NAME),
        frontend: resources.join(FRONTEND_RESOURCE_NAME),
        sdk: resources.join(SDK_RESOURCE_NAME),
        pg_bin: postgres.join("bin"),
        pg_lib: postgres.join("lib"),
    }
}

/// `~/Library/Application Support/<bundle id>` — the same path Tauri's
/// `app_data_dir()` returns for the client, computed from `$HOME` so the
/// service role (no `AppHandle`) agrees with the client.
fn app_data_dir_from_env() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support")
        .join(BUNDLE_IDENTIFIER))
}

/// Resolve the stable gateway port: `LUCIDOS_ENGINE_PORT` env override wins,
/// then the persisted `<app-data>/config/engine-port`, else the default
/// (written to that historical file name on first run so it stays stable and
/// user-editable).
fn resolve_engine_port(app_data: &Path) -> u16 {
    if let Some(p) = std::env::var("LUCIDOS_ENGINE_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
    {
        return p;
    }
    let cfg = app_data.join("config/engine-port");
    if let Ok(s) = std::fs::read_to_string(&cfg) {
        if let Ok(p) = s.trim().parse::<u16>() {
            if p != 0 {
                return p;
            }
        }
    }
    // First run: persist the default so the URL is stable and the user can edit it.
    let _ = std::fs::create_dir_all(app_data.join("config"));
    let _ = std::fs::write(&cfg, DEFAULT_ENGINE_PORT.to_string());
    DEFAULT_ENGINE_PORT
}

/// The stable gateway port for this install. Used by the mobile-access module
/// to build connect URLs. Falls back to the default if app-data can't be
/// resolved.
pub fn engine_port() -> u16 {
    app_data_dir_from_env()
        .map(|d| resolve_engine_port(&d))
        .unwrap_or(DEFAULT_ENGINE_PORT)
}

// ── Client role: ensure the service is up, then point the window at it ───────

/// Ensure the always-on service is running and point the main window at the
/// gateway. No-op in development. Runs the (possibly slow) ensure-and-wait on a
/// background thread so the window paints immediately; the window is navigated
/// to the gateway URL once it is healthy.
pub fn launch(app: &AppHandle) {
    if tauri::is_dev() {
        return;
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        let app_data = match app_data_dir_from_env() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[desktop] cannot resolve app_data_dir: {e}");
                return;
            }
        };
        let port = resolve_engine_port(&app_data);

        // Best-effort: install the LaunchAgent (if missing/stale) and start it.
        // Even if this fails, the service may already be running from login, so
        // fall through to the health wait regardless.
        if let Err(e) = ensure_service_installed_and_running(&app_data) {
            eprintln!("[desktop] ensure service running failed: {e}");
        }

        if !wait_for_health(port, ENGINE_HEALTH_TIMEOUT) {
            // The window stays on the bundled splash; surface the failure in logs.
            eprintln!("[desktop] gateway did not become healthy on port {port} within the timeout");
            return;
        }

        let url = format!("http://localhost:{port}");
        match (handle.get_webview_window("main"), url.parse::<tauri::Url>()) {
            (Some(win), Ok(parsed)) => {
                if let Err(e) = win.navigate(parsed) {
                    eprintln!("[desktop] failed to navigate window to gateway: {e}");
                }
            }
            _ => eprintln!("[desktop] no main window / bad gateway URL: {url}"),
        }
    });
}

/// Block until the gateway health endpoint answers 200, or the deadline passes.
/// The gateway serves health behind the sigil namespace (`/~/api/v1/health`,
/// ADR 0014) — a bare `/api/v1/health` would be resolved as a workspace slug.
fn wait_for_health(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if http_ok(port, "/~/api/v1/health") {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

// ── Service role: the launchd entry point ───────────────────────────────────

/// Headless launchd entry point (`Lucidos --service`). Boots the standalone
/// gateway on the stable port and supervises it, never touching AppKit/Tauri
/// (so no window, no dock icon). Returns the process exit code:
///  * `0` — graceful stop (SIGTERM from `bootout` / `kickstart -k`), or the
///    gateway exited and launchd's `KeepAlive` should respawn us.
///  * non-zero — boot failed; launchd respawns after `ThrottleInterval`.
pub fn run_service() -> i32 {
    install_stop_handlers();

    let app_data = match app_data_dir_from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[service] cannot resolve app_data_dir: {e}");
            return 1;
        }
    };
    let resources = match resource_dir_from_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[service] cannot resolve resource dir: {e}");
            return 1;
        }
    };
    let port = resolve_engine_port(&app_data);

    let mut svc = match spawn_gateway(&resources, &app_data, port) {
        Ok(svc) => svc,
        Err(e) => {
            eprintln!("[service] failed to start workspace gateway: {e}");
            return 1;
        }
    };
    if !wait_for_health(port, ENGINE_HEALTH_TIMEOUT) {
        eprintln!("[service] gateway did not become healthy on port {port}");
        svc.shutdown(&resources, &app_data);
        return 1;
    }
    eprintln!("[service] gateway healthy on port {port}; supervising");

    // Supervise: exit when asked to stop, or when the gateway dies (launchd's
    // KeepAlive respawns us, re-launching the gateway, which re-adopts any
    // already-running workspace engines).
    loop {
        if SERVICE_STOP.load(Ordering::SeqCst) {
            eprintln!("[service] stop requested; shutting down");
            break;
        }
        match svc.gateway.try_wait() {
            Ok(Some(status)) => {
                eprintln!("[service] gateway exited ({status}); shutting down for respawn");
                break;
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("[service] failed to poll gateway: {e}");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    svc.shutdown(&resources, &app_data);
    0
}

/// Install SIGTERM/SIGINT handlers that flip [`SERVICE_STOP`] so the supervise
/// loop can tear the stack down gracefully before launchd's SIGKILL deadline.
#[cfg(unix)]
fn install_stop_handlers() {
    extern "C" fn on_stop(_sig: libc::c_int) {
        SERVICE_STOP.store(true, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_stop as *const () as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
fn install_stop_handlers() {}

// ── LaunchAgent management (client role) ────────────────────────────────────

/// Install/update the plist (if missing or stale) and ensure the service is
/// loaded and running.
///
/// Note: the plist captures `current_exe()`. A SIGNED + NOTARIZED app in
/// `/Applications` has a stable path. An UNSIGNED local test build run from
/// Downloads can be Gatekeeper *app-translocated* to a random read-only mount,
/// so the captured path would later vanish — move the `.app` into
/// `/Applications` (or sign it) before relying on the service across reboots.
fn ensure_service_installed_and_running(app_data: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let changed = install_or_update_plist(&exe, app_data)?;

    if changed && is_service_loaded() {
        // A rewritten definition only takes effect after a reload. Remove the
        // old one, then re-bootstrap UNCONDITIONALLY — don't branch on a fresh
        // `is_service_loaded()` here, which can still report the just-booted-out
        // job as loaded before launchd settles and would kickstart the stale
        // definition. `bootstrap_service` tolerates an already-loaded job.
        let _ = bootout_service();
        bootstrap_service()?;
    } else if !is_service_loaded() {
        bootstrap_service()?;
    } else {
        // Loaded and unchanged but possibly not running (e.g. RunAtLoad already
        // fired and the job idled out). A bare kickstart starts it without
        // killing a healthy one.
        let _ = kickstart_service(false);
    }
    Ok(())
}

/// `~/Library/LaunchAgents/<label>.plist`.
fn plist_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist")))
}

/// Minimal XML text escaping for paths embedded in the plist.
fn xml_escape(p: &Path) -> String {
    p.to_string_lossy()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The plist that runs `Lucidos --service` at login, restarts it on crash, and
/// logs to the app-data `logs/` dir.
fn desired_plist(exe: &Path, app_data: &Path) -> String {
    let logs = app_data.join("logs");
    let out = logs.join("engine-service.out.log");
    let err = logs.join("engine-service.err.log");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--service</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
        label = LAUNCH_AGENT_LABEL,
        exe = xml_escape(exe),
        out = xml_escape(&out),
        err = xml_escape(&err),
    )
}

/// Write the plist if missing or different from desired. Returns true if it was
/// (re)written.
fn install_or_update_plist(exe: &Path, app_data: &Path) -> io::Result<bool> {
    let desired = desired_plist(exe, app_data);
    let path = plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(app_data.join("logs"))?;
    if std::fs::read_to_string(&path).ok().as_deref() == Some(desired.as_str()) {
        return Ok(false);
    }
    std::fs::write(&path, desired)?;
    Ok(true)
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// `gui/<uid>` — the per-user launchd domain.
fn service_domain() -> String {
    format!("gui/{}", current_uid())
}

/// `gui/<uid>/<label>` — the service's launchd target.
fn service_target() -> String {
    format!("gui/{}/{}", current_uid(), LAUNCH_AGENT_LABEL)
}

fn launchctl(args: &[&str]) -> io::Result<std::process::Output> {
    Command::new("launchctl").args(args).output()
}

/// True if the service is bootstrapped into the user's launchd domain.
fn is_service_loaded() -> bool {
    launchctl(&["print", &service_target()])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn bootstrap_service() -> io::Result<()> {
    let plist = plist_path()?;
    let plist_s = plist.to_string_lossy().to_string();
    let out = launchctl(&["bootstrap", &service_domain(), &plist_s])?;
    if !out.status.success() {
        // Idempotent re-bootstrap of an already-loaded service is fine; only
        // surface a genuine failure to load.
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !is_service_loaded() {
            return Err(io::Error::other(format!(
                "launchctl bootstrap failed: {}",
                stderr.trim()
            )));
        }
    }
    Ok(())
}

fn bootout_service() -> io::Result<()> {
    let out = launchctl(&["bootout", &service_target()])?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "launchctl bootout failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

fn kickstart_service(kill: bool) -> io::Result<()> {
    let target = service_target();
    let args: Vec<&str> = if kill {
        vec!["kickstart", "-k", &target]
    } else {
        vec!["kickstart", &target]
    };
    let out = launchctl(&args)?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "launchctl kickstart failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Restart the gateway service in place (`launchctl kickstart -k`). The
/// supervisor catches the SIGTERM, tears the stack down gracefully, and
/// launchd respawns it. Used by the packaged "Restart" control. In development
/// there is no service, so this returns an error the caller can show (the
/// frontend only routes here in packaged mode, so it shouldn't be hit).
pub fn restart_service() -> Result<(), String> {
    if tauri::is_dev() {
        return Err("Gateway service restart is only available in a packaged build".to_string());
    }
    kickstart_service(true).map_err(|e| e.to_string())
}

/// Stop the always-on service entirely (`launchctl bootout`) — the explicit
/// "Quit Lucidos" path. Removes the agent so it won't respawn; the next GUI
/// launch re-installs and re-bootstraps it. No-op in development (no service).
pub fn stop_service() {
    if tauri::is_dev() {
        return;
    }
    if let Err(e) = bootout_service() {
        eprintln!("[desktop] stop service failed: {e}");
    }
}

// ── Gateway boot (service role) ─────────────────────────────────────────────

/// Spawn the bundled standalone `lucidos-gateway` (ADR 0014) pointed at the OS
/// app-data dir, configured for the EMBEDDED Postgres backend (bundled
/// `initdb`/`pg_ctl`). The gateway owns the rest: it reads/creates the workspace
/// registry, provisions one embedded shared Postgres cluster with one database
/// per workspace, and spawns each workspace's engine (by path via
/// `LUCIDOS_ENGINE_BIN`, inheriting `LUCIDOS_STATIC_DIR` /
/// `LUCIDOS_SDK_DIR` / `LUCIDOS_BOOT_WITHOUT_PROVIDER` from this env), and
/// reverse-proxies `/<slug>/`. First run auto-creates a `default` workspace.
/// `LUCIDOS_BOOT_WITHOUT_PROVIDER` lets engines boot before the user has
/// configured a provider — into a clear no-provider onboarding state, NOT mock
/// output (see engine main.rs).
fn spawn_gateway(resources: &Path, app_data: &Path, port: u16) -> io::Result<GatewayService> {
    let bundle = bundled_resources(resources);

    // The embedding model (fastembed) caches its ONNX model under
    // `FASTEMBED_CACHE_DIR` (default: `.fastembed_cache` relative to CWD). Under
    // launchd the CWD is read-only `/`, so pin it to a writable, update-surviving
    // dir under app-data, and give the gateway a writable CWD. The gateway
    // overrides this per-workspace when it spawns each engine.
    let fastembed_cache = app_data.join("fastembed");
    std::fs::create_dir_all(&fastembed_cache)?;
    std::fs::create_dir_all(app_data.join("config"))?;

    let gateway = Command::new(&bundle.gateway_bin)
        .current_dir(app_data)
        .env("LUCIDOS_API_PORT", port.to_string())
        .env("LUCIDOS_GATEWAY_DATA", app_data)
        .env("LUCIDOS_GATEWAY_PG_BACKEND", "embedded")
        .env("LUCIDOS_PG_BIN_DIR", &bundle.pg_bin)
        .env("LUCIDOS_PG_LIB_DIR", &bundle.pg_lib)
        .env("LUCIDOS_ENGINE_BIN", &bundle.engine_bin)
        .env("LUCIDOS_STATIC_DIR", &bundle.frontend)
        .env("LUCIDOS_SDK_DIR", &bundle.sdk)
        .env("FASTEMBED_CACHE_DIR", &fastembed_cache)
        .env("LUCIDOS_BOOT_WITHOUT_PROVIDER", "1")
        .spawn()?;
    Ok(GatewayService { gateway })
}

/// Minimal dependency-free HTTP GET that returns true on a `200` status line.
fn http_ok(port: u16, path: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    let first = buf.lines().next().unwrap_or_default();
    first.contains(" 200 ") || first.ends_with(" 200")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_escapes_markup_chars() {
        let p = Path::new("/Users/a&b/<x>/Lucidos");
        assert_eq!(xml_escape(p), "/Users/a&amp;b/&lt;x&gt;/Lucidos");
    }

    #[test]
    fn desired_plist_runs_service_role_with_keepalive() {
        let exe = Path::new("/Applications/Lucidos.app/Contents/MacOS/Lucidos");
        let app_data = Path::new("/Users/me/Library/Application Support/com.lucidos.app");
        let plist = desired_plist(exe, app_data);

        assert!(plist.contains(&format!("<string>{LAUNCH_AGENT_LABEL}</string>")));
        assert!(plist.contains("<string>/Applications/Lucidos.app/Contents/MacOS/Lucidos</string>"));
        assert!(plist.contains("<string>--service</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        // Logs land under the app-data dir, not next to the bundle.
        assert!(plist.contains("Application Support/com.lucidos.app/logs/engine-service.out.log"));
    }

    #[test]
    fn resource_dir_for_exe_matches_macos_bundle_layout() {
        let exe = Path::new("/Applications/Lucidos.app/Contents/MacOS/Lucidos");
        assert_eq!(
            resource_dir_for_exe(exe).unwrap(),
            PathBuf::from("/Applications/Lucidos.app/Contents/Resources")
        );
    }

    #[test]
    fn bundled_resource_paths_match_build_dmg_contract() {
        let resources = Path::new("/Applications/Lucidos.app/Contents/Resources");
        let bundle = bundled_resources(resources);

        assert_eq!(bundle.gateway_bin, resources.join(GATEWAY_RESOURCE_NAME));
        assert_eq!(bundle.engine_bin, resources.join(ENGINE_RESOURCE_NAME));
        assert_eq!(bundle.frontend, resources.join(FRONTEND_RESOURCE_NAME));
        assert_eq!(bundle.sdk, resources.join(SDK_RESOURCE_NAME));
        assert_eq!(
            bundle.pg_bin,
            resources.join(POSTGRES_RESOURCE_NAME).join("bin")
        );
        assert_eq!(
            bundle.pg_lib,
            resources.join(POSTGRES_RESOURCE_NAME).join("lib")
        );
    }

    #[test]
    fn embedded_pg_stop_command_uses_fast_mode_against_the_data_dir() {
        let cmd = embedded_pg_stop_command(
            Path::new("/r/postgres/bin"),
            Path::new("/r/postgres/lib"),
            Path::new("/d/pgdata"),
        );
        assert!(cmd
            .get_program()
            .to_string_lossy()
            .ends_with("postgres/bin/pg_ctl"));
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(args, vec!["-D", "/d/pgdata", "-m", "fast", "-w", "stop"]);
    }

    #[test]
    fn stop_embedded_postgres_is_a_noop_without_a_provisioned_cluster() {
        // No <app-data>/pgdata/PG_VERSION → must early-return without trying to
        // spawn a (here, nonexistent) pg_ctl, so this call simply does nothing.
        let dir = std::env::temp_dir().join(format!("lucidos-nopg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        stop_embedded_postgres(Path::new("/nonexistent/resources"), &dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_engine_port_defaults_then_persists_and_reads_back() {
        // Determinism: ignore any ambient override for this test.
        std::env::remove_var("LUCIDOS_ENGINE_PORT");

        let dir = std::env::temp_dir().join(format!("lucidos-port-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // First call: no file → default, and it persists the default.
        assert_eq!(resolve_engine_port(&dir), DEFAULT_ENGINE_PORT);
        let cfg = dir.join("config/engine-port");
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap().trim(),
            DEFAULT_ENGINE_PORT.to_string()
        );

        // A persisted custom value is read back verbatim.
        std::fs::write(&cfg, "6123").unwrap();
        assert_eq!(resolve_engine_port(&dir), 6123);

        // A garbage / zero value falls back to the default.
        std::fs::write(&cfg, "nope").unwrap();
        assert_eq!(resolve_engine_port(&dir), DEFAULT_ENGINE_PORT);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
