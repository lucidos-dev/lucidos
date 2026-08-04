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
//!    registered workspace and reverse-proxies `/<slug>/` (first run creates no
//!    workspace; the smart root serves the picker). No window, no AppKit. On
//!    crash launchd respawns the service (the
//!    new gateway re-adopts already-running engines); on `launchctl bootout` (the
//!    explicit "Quit and Stop Background Service") it tears the whole stack down and
//!    stays stopped.
//!  * **Client** (the GUI app the user double-clicks): [`launch`] ensures the
//!    service is installed + running, waits for `/~/api/v1/health` (the gateway),
//!    then points the window at it (the gateway serves the workspace picker behind
//!    the sigil namespace `/~/`). Closing the window and Cmd+Q only dismiss the
//!    window — the client stays resident in the menu bar and the service keeps
//!    running; only the menu-bar "Quit and Stop Background Service" tears it down.
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

/// One health-poll cycle in the client's start-and-navigate loop ([`launch`]):
/// how long to wait for `/~/api/v1/health` before re-ensuring the service and
/// polling again. The loop NEVER gives up, so this only bounds how often a
/// crashed/idle service is re-kickstarted while the window waits.
const HEALTH_ENSURE_CYCLE: Duration = Duration::from_secs(30);

/// How often the desktop process refreshes the dock-icon badge from the gateway's
/// aggregate unread total (macOS only). Independent of the webview's own polling
/// so the badge is correct whichever page (picker or a workspace) is loaded.
#[cfg(target_os = "macos")]
const DOCK_BADGE_POLL_INTERVAL: Duration = Duration::from_secs(5);

const GATEWAY_RESOURCE_NAME: &str = "lucidos-gateway";
const ENGINE_RESOURCE_NAME: &str = "lucidos-engine";
const FRONTEND_RESOURCE_NAME: &str = "frontend";
const SDK_RESOURCE_NAME: &str = "sdk";
const POSTGRES_RESOURCE_NAME: &str = "postgres";
/// The engine-shipped reference knowhow dir (`system-knowhow/`), staged as the
/// 7th resource. The engine resolves it by absolute path via
/// `LUCIDOS_SYSTEM_KNOWHOW_DIR` (set in `spawn_gateway`); without it a packaged
/// install silently loses the entire engine-facing reference set.
const SYSTEM_KNOWHOW_RESOURCE_NAME: &str = "system-knowhow";
/// The `lucidos` CLI binary (cargo package `lucidos-cli` → bin `lucidos`),
/// staged flat next to the engine. Backs the coding-agent permission/question
/// MCP servers, the CC hooks, and chat-script `lucidos …` calls; the engine
/// resolves it by absolute path via `LUCIDOS_CLI_BIN` (set in `spawn_gateway`).
const CLI_RESOURCE_NAME: &str = "lucidos";

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
    /// This is the PERMANENT-stop teardown — "Quit and Stop Background Service"
    /// (`bootout`) and the supervised-exit path both route here. It is NOT reached by the gateway's
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
        // Record what we are about to stop so the NEXT gateway boot can bring it
        // back. This teardown runs for a restart (`launchctl kickstart -k`, which
        // is what the Restart control and the updater use) and for a crash
        // respawn, not just for a permanent stop, and in those cases the user
        // never asked for their workspaces to go away. Suppressed when the record
        // already says `quit`. See `record_workspaces_to_restore`.
        let stopped = stop_workspace_engines(app_data);
        record_workspaces_to_restore(app_data, &stopped);
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
/// `<app-data>/workspaces/<id>/.lucidos/engine.pid`), returning the ids of the
/// ones that were actually alive. Used on a full service stop
/// ("Quit and Stop Background Service"); the gateway leaves them running on its own
/// SIGUSR1 so they
/// can be re-adopted across a gateway restart, but an explicit stop tears the
/// whole stack down.
///
/// The returned ids are what the next boot owes the user
/// ([`record_workspaces_to_restore`]), which is why liveness is checked rather
/// than trusting the pidfile: a stale pidfile from an engine that died on its own
/// would otherwise make a restart "restore" a workspace nobody was running.
fn stop_workspace_engines(app_data: &Path) -> Vec<String> {
    let workspaces = app_data.join("workspaces");
    let Ok(entries) = std::fs::read_dir(&workspaces) else {
        return Vec::new();
    };
    let mut stopped = Vec::new();
    for entry in entries.flatten() {
        let pidfile = entry.path().join(".lucidos/engine.pid");
        let Ok(contents) = std::fs::read_to_string(&pidfile) else {
            continue;
        };
        if let Ok(pid) = contents.trim().parse::<i32>() {
            #[cfg(unix)]
            {
                // SAFETY: signal 0 checks for the process's existence without
                // delivering anything; SIGUSR1 then asks a live engine to stop.
                // A dead pid returns ESRCH from both.
                let alive = unsafe { libc::kill(pid, 0) } == 0;
                unsafe {
                    libc::kill(pid, libc::SIGUSR1);
                }
                if alive {
                    if let Some(id) = entry.file_name().to_str() {
                        stopped.push(id.to_string());
                    }
                }
            }
            let _ = std::fs::remove_file(&pidfile);
        }
    }
    stopped
}

// ── What the next boot owes the user ────────────────────────────────────────

/// The record the next gateway boot reads to decide which workspaces to bring
/// back: `<app-data>/.next-boot.json`. The reader is `next_boot.rs` in the
/// gateway crate, which this crate does not link, so the two spell the same
/// filename and the same JSON and each pins it with a test.
const NEXT_BOOT_FILE: &str = ".next-boot.json";

fn next_boot_path(app_data: &Path) -> PathBuf {
    app_data.join(NEXT_BOOT_FILE)
}

/// The `{"quit": true}` body: the last teardown was deliberate, restore nothing.
const NEXT_BOOT_QUIT: &str = "{\"quit\":true}";

/// Note down the workspaces the teardown just stopped, so the next gateway boot
/// starts them again.
///
/// The point is that a restart must return what it took. `launchctl kickstart -k`
/// (the Restart control, and the updater's service restart) and a crash respawn
/// both run the same teardown as a permanent stop, and the gateway that comes up
/// afterwards re-adopts only engines that survived, of which there are none. So
/// without this the workspace the user was sitting in stays stopped, and its open
/// page cannot even wake it: API traffic never lazy-starts a workspace, because
/// that guard is what makes the picker's Stop button stick.
///
/// Skipped when the record already says `quit`: [`stop_service`] writes that
/// BEFORE it signals launchd, so "Quit and Stop Background Service" stays quiet
/// and the next launch is as lazy as it ever was.
///
/// Best-effort. Failing to write it costs a restart its workspaces, which is
/// today's behaviour, and must never take the teardown down with it.
fn record_workspaces_to_restore(app_data: &Path, ids: &[String]) {
    let path = next_boot_path(app_data);
    if quit_was_declared(&path) {
        return; // A deliberate stop: leave the quit marker for the reader to clear.
    }
    if ids.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    match serde_json::to_string(&serde_json::json!({ "restore": ids })) {
        Ok(body) => {
            if let Err(e) = std::fs::write(&path, body) {
                eprintln!("[service] could not record workspaces to restore: {e}");
            }
        }
        Err(e) => eprintln!("[service] could not build the next-boot record: {e}"),
    }
}

/// Does the record on disk say a deliberate stop is under way?
///
/// Parsed rather than substring-matched: a workspace can legitimately be called
/// `quit`, and `{"restore":["quit"]}` must not read as one.
fn quit_was_declared(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|record| record.get("quit")?.as_bool())
        .unwrap_or(false)
}

/// Declare that the teardown about to happen is a deliberate full stop, so
/// [`record_workspaces_to_restore`] writes nothing and the next launch starts
/// nothing.
///
/// Written BEFORE the `bootout` that triggers the teardown, which is what makes
/// the ordering structural: deleting the record afterwards would be a race
/// against how synchronously `launchctl bootout` returns.
fn declare_quit_intent(app_data: &Path) {
    if let Err(e) = std::fs::write(next_boot_path(app_data), NEXT_BOOT_QUIT) {
        eprintln!("[service] could not record the quit intent: {e}");
    }
}

/// Drop the record, for when the teardown it was written for never happened.
/// Only the gateway's boot normally consumes it, so an intent whose teardown
/// fell through has to be taken back here or it silences the next real one.
fn clear_next_boot_record(app_data: &Path) {
    match std::fs::remove_file(next_boot_path(app_data)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("[service] could not clear the next-boot record: {e}"),
    }
}

// ── Path resolution (shared by both roles) ──────────────────────────────────

/// Resolve the bundle's `Resources` dir relative to the executable. In a
/// macOS `.app` the executable is `Contents/MacOS/lucidos-app` (Tauri names the
/// bundle from `productName` but the binary inside it from the crate, so the two
/// differ) and resources live in `Contents/Resources/`, which is exactly what the
/// client's `resource_dir()` returns, so the service resolves the same bundle.
fn resource_dir_from_exe() -> io::Result<PathBuf> {
    resource_dir_for_exe(&std::env::current_exe()?)
}

fn resource_dir_for_exe(exe: &Path) -> io::Result<PathBuf> {
    // <Contents>/MacOS/lucidos-app -> <Contents>/MacOS -> <Contents>
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
    cli_bin: PathBuf,
    frontend: PathBuf,
    sdk: PathBuf,
    system_knowhow: PathBuf,
    pg_bin: PathBuf,
    pg_lib: PathBuf,
}

fn bundled_resources(resources: &Path) -> BundledResources {
    let postgres = resources.join(POSTGRES_RESOURCE_NAME);
    BundledResources {
        gateway_bin: resources.join(GATEWAY_RESOURCE_NAME),
        engine_bin: resources.join(ENGINE_RESOURCE_NAME),
        cli_bin: resources.join(CLI_RESOURCE_NAME),
        frontend: resources.join(FRONTEND_RESOURCE_NAME),
        sdk: resources.join(SDK_RESOURCE_NAME),
        system_knowhow: resources.join(SYSTEM_KNOWHOW_RESOURCE_NAME),
        pg_bin: postgres.join("bin"),
        pg_lib: postgres.join("lib"),
    }
}

/// `~/Library/Application Support/<bundle id>` — the same path Tauri's
/// `app_data_dir()` returns for the client, computed from `$HOME` so the
/// service role (no `AppHandle`) agrees with the client. `pub` so the uninstall
/// command resolves the exact same data dir the service uses.
pub fn app_data_dir_from_env() -> io::Result<PathBuf> {
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

// ── The gateway origin and its ACL capability ────────────────────────────────

/// The URL a packaged client window is pointed at: the always-on gateway on this
/// install's stable port. The ONE place this string is built, because the
/// window's resulting origin has to match [`gateway_capability`]'s URL pattern
/// exactly — if the two ever drift, every IPC call from that window is denied.
pub fn gateway_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// Permissions granted on the gateway origin. Deliberately identical to the set
/// `capabilities/default.json` grants on the LOCAL app URL: it is the same
/// frontend, just reached over http instead of off the bundled assets. Kept in
/// step by `gateway_capability_grants_what_the_default_capability_grants`.
const GATEWAY_PERMISSIONS: &[&str] = &[
    "allow-app-ipc",
    "core:default",
    "core:webview:allow-create-webview",
    "core:webview:allow-webview-close",
    "core:webview:allow-set-webview-position",
    "core:webview:allow-set-webview-size",
    "core:webview:allow-webview-show",
    "core:webview:allow-webview-hide",
    "core:webview:allow-set-webview-focus",
    "dialog:default",
    "updater:default",
];

/// The ACL capability that lets the packaged window talk to the app at all.
///
/// `frontendDist` makes the Tauri app URL `tauri://localhost`, but [`launch`]
/// navigates the window to the gateway ([`gateway_url`]). Since tauri 2.11
/// (`Webview::on_message`: `plugin_command.is_some() || has_app_acl_manifest ||
/// !is_local`) every IPC request from a non-local URL is ACL-checked as
/// `Origin::Remote`, so without this capability EVERY command — ours and the
/// plugins' — is rejected with `Command <x> not allowed by ACL`. That is the
/// 2.10.2 → 2.11.4 regression this exists to close; the upstream change is
/// deliberate hardening, so we grant the origin explicitly rather than pin the
/// old version.
///
/// Scoping, tightest the ACL schema allows:
///  * the URL pattern carries the **resolved port**, not `localhost:*` — the port
///    is per-install (`<app-data>/config/engine-port`), so it can only be pinned
///    at runtime, and a wildcard would hand IPC to any other local HTTP server
///    the window could be navigated to.
///  * `webviews`, never `windows` — a `windows: ["main"]` entry enables a
///    capability on every webview of that window, and the `url-preview-*`
///    webviews showing arbitrary third-party sites live inside the main window.
///  * `local(false)` — the local app URL is `capabilities/default.json`'s job;
///    this capability speaks only for the gateway origin.
///
/// **What is in the grant set, spelled out (F12).** [`GATEWAY_PERMISSIONS`]
/// includes `updater:default`, and that pulls in `plugin:updater|download_and_install`.
/// So the honest statement of what this capability grants is not "the app can
/// update itself" but "anything answering on `http://localhost:<port>` can drive
/// a signed bundle swap and a full stack restart", and those are different
/// sentences. Nothing else in the set reaches as far.
///
/// The residual is narrow and it is ACCEPTED rather than unnoticed. The origin is
/// plain HTTP on loopback with no authentication, so a local process that bound
/// the port BEFORE the gateway did would receive this window's IPC. Three things
/// keep that from being reachable in practice: [`launch`] navigates only after
/// the gateway answers its health check, the gateway holds the port for the life
/// of the service, and a squatter would have to win the port on a machine where
/// the user already trusts every local process with their workspace data anyway.
/// The two obvious hardenings both cost more than they buy: dropping
/// `updater:default` would move the update path off the window that shows it, and
/// authenticating loopback would put a shared secret in a page the gateway itself
/// serves.
///
/// Read this before widening [`GATEWAY_PERMISSIONS`]: the question to ask of a
/// new entry is not whether the frontend needs it, but what it hands to whoever
/// answers on that port.
pub(crate) fn gateway_capability(port: u16) -> tauri::ipc::CapabilityBuilder {
    GATEWAY_PERMISSIONS.iter().fold(
        tauri::ipc::CapabilityBuilder::new("gateway")
            .local(false)
            .remote(gateway_url(port))
            .webviews(["main", "window-*"]),
        |capability, permission| capability.permission(*permission),
    )
}

// ── Client role: ensure the service is up, then point the window at it ───────

/// Ensure the always-on service is running and point the main window at the
/// gateway. No-op in development. Runs the (possibly slow) ensure-and-wait on a
/// background thread so the window paints immediately; the window is navigated
/// to the gateway URL once it is healthy.
pub fn launch(app: &AppHandle, nudge_rx: std::sync::mpsc::Receiver<()>) {
    if tauri::is_dev() {
        // No dock-badge thread in dev (unbundled; dev uses the browser) — drop the
        // receiver so the managed sender's `send` is a harmless no-op.
        drop(nudge_rx);
        return;
    }

    // Dock-icon badge: mirror the gateway's aggregate unread total (across running
    // workspaces) onto the app icon. Its own thread — independent of the
    // service/health/navigate flow below and of whichever page the webview shows —
    // so the desktop badge always reflects the TOTAL, not just the active
    // workspace. macOS only (the dock tile is a macOS concept). The AppKit write
    // is marshalled to the main thread; the fetch tolerates the gateway not being
    // up yet (returns None until it answers).
    //
    // Event-driven AND polled: the loop recomputes the instant it's NUDGED (the
    // active workspace's webview signals `nudge_dock_badge` when a notification SSE
    // arrives — read in-app or from another device) so the badge updates without
    // waiting for the next tick; the periodic `DOCK_BADGE_POLL_INTERVAL` tick is
    // the safety net that also catches BACKGROUND-workspace changes (whose SSE this
    // webview never sees).
    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        std::thread::spawn(move || {
            let port = engine_port();
            let mut last: Option<u64> = None;
            loop {
                if let Some(total) = fetch_unread_total(port) {
                    // Only touch AppKit when the value actually changed — avoids a
                    // main-thread hop when nothing moved.
                    if last != Some(total) {
                        last = Some(total);
                        let h = handle.clone();
                        let _ = handle.run_on_main_thread(move || {
                            // Routes to the Dock badge (Regular) or the menu-bar
                            // tray title (menu-bar-only Accessory) per activation state.
                            crate::apply_unread_indicator(&h, total);
                        });
                    }
                }
                // Wait for the next tick OR a nudge, whichever comes first. Drain
                // any extra queued nudges so a flurry of notification SSEs in quick
                // succession (e.g. the create-then-auto-read pair, or several
                // arriving at once) collapses to one recompute. A dropped sender
                // (Disconnected) degrades to a plain timed poll.
                match nudge_rx.recv_timeout(DOCK_BADGE_POLL_INTERVAL) {
                    Ok(()) => while nudge_rx.try_recv().is_ok() {},
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(DOCK_BADGE_POLL_INTERVAL);
                    }
                }
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    drop(nudge_rx); // dock badge is macOS-only; no consumer elsewhere

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

        // Grant the gateway origin its IPC permissions BEFORE anything can put a
        // window on it — the ACL is consulted per invoke, so registering it here
        // covers the navigation below and every later New Window (which inherits
        // the main window's URL).
        //
        // The realistic failure — a permission identifier that doesn't resolve —
        // is caught at TEST time, not here: `acl_tests` resolves this exact
        // capability through tauri's own resolver, and tauri would `unwrap` the
        // resolve error rather than hand us an `Err`. So this arm is a backstop.
        // It logs and still navigates on purpose: a reachable UI with a dead
        // bridge beats stranding the user on the "Starting Lucidos…" splash with
        // no explanation, and the page's own IPC telemetry (utils/ipcHealth.ts)
        // then reports the dead bridge to the engine log — where it is visible
        // without a debugger, unlike this line.
        if let Err(e) = handle.add_capability(gateway_capability(port)) {
            eprintln!(
                "[desktop] FAILED to register the gateway ACL capability for {} — every Tauri IPC \
                 call from the window will be rejected by the ACL: {e}",
                gateway_url(port)
            );
        }

        // Keep the service up and navigate the window the moment the gateway is
        // healthy — NEVER permanently give up. A slow post-forced-shutdown start
        // (Postgres WAL crash recovery + embedding warmup) or a transient
        // crash-respawn can exceed a single wait; retrying + re-ensuring means the
        // window resolves whenever the service comes up instead of stranding the
        // user on the bundled "Starting Lucidos…" splash (main.tsx). Each cycle
        // re-ensures the LaunchAgent with a bare kickstart — a no-op on a
        // still-starting service, a restart on a crashed/idle one, so it never
        // interrupts a slow-but-progressing start — then polls health for one
        // bounded cycle. `wait_for_health` sleeps between attempts, so this can't
        // busy-loop.
        loop {
            if let Err(e) = ensure_service_installed_and_running(&app_data) {
                eprintln!("[desktop] ensure service running failed: {e}");
            }
            if wait_for_health(port, HEALTH_ENSURE_CYCLE) {
                break;
            }
            eprintln!(
                "[desktop] gateway not healthy yet on port {port}; re-ensuring service and retrying"
            );
        }

        let url = gateway_url(port);
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
/// "Quit and Stop Background Service" path (menu-bar item / app menu). Removes the
/// agent so it won't respawn; the next GUI launch re-installs and re-bootstraps
/// it. No-op in development (no service).
///
/// This is the ONE teardown that means *stay down*, so it declares that first:
/// the service's own teardown otherwise records its workspaces for the next boot
/// to restore, which is right for a restart and wrong here. Declaring before the
/// `bootout` (rather than clearing the record after it) is what keeps the
/// ordering structural.
pub fn stop_service() {
    if tauri::is_dev() {
        return;
    }
    let app_data = match app_data_dir_from_env() {
        Ok(app_data) => {
            declare_quit_intent(&app_data);
            Some(app_data)
        }
        Err(e) => {
            eprintln!("[desktop] cannot resolve app-data for the quit intent: {e}");
            None
        }
    };
    if let Err(e) = bootout_service() {
        eprintln!("[desktop] stop service failed: {e}");
        // The service is still up, so the teardown this intent was written for
        // never happens and nothing consumes the record. Left behind, it would
        // silence the NEXT restart's restore list and put us straight back in the
        // bug this exists to fix, so take it back.
        if let Some(app_data) = &app_data {
            clear_next_boot_record(app_data);
        }
    }
}

// ── Relaunching the client (client role) ────────────────────────────────────

/// Poll interval, in seconds, for the relaunch watcher's `kill -0` probe. A
/// normal exit lands within a few probes; the interval only has to be short
/// enough that the relaunch feels immediate.
#[cfg(target_os = "macos")]
const RELAUNCH_POLL_SECONDS: &str = "0.1";

/// How many [`RELAUNCH_POLL_SECONDS`] probes the watcher makes before giving up
/// on the client ever exiting: roughly five minutes.
///
/// This bounds the WATCHER's life, not the relaunch: a detached shell that could
/// loop forever is the failure it exists to prevent. It is deliberately far
/// longer than a shutdown takes, because giving up is giving up for good, and
/// the launch is conditional on the client actually being gone. Launching on the
/// timeout instead would spend the one relaunch on a still-live process (`open`
/// would just activate it) and leave nothing to bring the client back when it
/// finally did exit. A client still running after five minutes is not shutting
/// down, and it needs no relaunch: it is on screen.
#[cfg(target_os = "macos")]
const RELAUNCH_WAIT_PROBES: u32 = 3000;

/// Arrange for LaunchServices to relaunch this app once THIS process has
/// exited. `Ok` means it is arranged and the caller MUST exit; `Err` means it
/// isn't, and the caller must fall back to respawning the executable itself.
///
/// **Why LaunchServices instead of spawning the executable again.** Tauri's
/// `process::restart` (and our own `restart_process`) fork/exec the binary
/// directly, which never asks the system to activate the new instance. The only
/// way such an instance lands in front is by inheriting the front slot from its
/// dying parent, and that is a race it loses whenever it registers with the
/// window server *after* the parent is gone. The 0.20 → 0.20.1 update on
/// 2026-08-03 lost it by ~280ms: the old client died still frontmost, the front
/// slot went to the next app, and the updated client sat behind everything until
/// the user Cmd+Tabbed to it. `open` has LaunchServices launch the app the way a
/// double-click does, which grants activation outright, and the watcher runs it
/// strictly after we are gone, so there is no front slot to inherit and no race
/// to lose.
///
/// **Why it waits for our exit** rather than launching straight away: `open`
/// against a running app activates the running (here: dying) instance instead of
/// launching a new one, and `open -n` would leave two clients overlapping. The
/// wait is what keeps this to exactly one instance.
///
/// Development needs no special case: an unbundled `tauri dev` binary has no
/// enclosing `.app`, so the resolution below fails and the caller falls back.
#[cfg(target_os = "macos")]
pub fn schedule_relaunch_after_exit() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("resolve this executable: {e}"))?;
    let bundle = app_bundle_root_from_exe(&exe)
        .ok_or_else(|| format!("{} is not inside a .app bundle", exe.display()))?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let script = relaunch_watcher_script(std::process::id(), &bundle, &args)?;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(&script)
        .spawn()
        .map_err(|e| format!("spawn the relaunch watcher: {e}"))?;
    Ok(())
}

/// macOS is the only packaged GUI shape Lucidos ships, so there is nothing to
/// arrange anywhere else and the caller keeps its own respawn.
#[cfg(not(target_os = "macos"))]
pub fn schedule_relaunch_after_exit() -> Result<(), String> {
    Err("a LaunchServices relaunch is macOS-only".to_string())
}

/// The watcher script: wait (bounded) for `pid` to disappear and then, ONLY if
/// it did, hand `bundle` to LaunchServices, forwarding `args` as the new
/// instance's arguments.
///
/// The launch is guarded by a second `kill -0` rather than following the loop
/// unconditionally, because the loop can also end at its ceiling. Launching then
/// would aim `open` at a process that is still alive, which merely activates it,
/// and the watcher would be gone by the time the client actually exited.
///
/// Pure, so the bound, the ordering (wait *then* launch) and the quoting are
/// unit-tested rather than eyeballed. Errors on a path or argument that isn't
/// valid UTF-8: it cannot be quoted into a shell word without corrupting it, and
/// the caller's fallback passes `OsString`s through faithfully.
#[cfg(target_os = "macos")]
fn relaunch_watcher_script(
    pid: u32,
    bundle: &Path,
    args: &[std::ffi::OsString],
) -> Result<String, String> {
    let bundle = bundle
        .to_str()
        .ok_or_else(|| format!("bundle path is not valid UTF-8: {}", bundle.display()))?;
    let mut launch = format!("exec /usr/bin/open -a {}", sh_quote(bundle));
    if !args.is_empty() {
        launch.push_str(" --args");
        for arg in args {
            let arg = arg
                .to_str()
                .ok_or_else(|| "an argument is not valid UTF-8".to_string())?;
            launch.push(' ');
            launch.push_str(&sh_quote(arg));
        }
    }
    Ok(format!(
        "i=0; while [ $i -lt {RELAUNCH_WAIT_PROBES} ] && kill -0 {pid} 2>/dev/null; \
         do sleep {RELAUNCH_POLL_SECONDS}; i=$((i+1)); done; \
         kill -0 {pid} 2>/dev/null || {launch}"
    ))
}

/// Quote `s` as one POSIX shell word. Single quotes take everything literally,
/// so only an embedded single quote needs work: close, escape it, reopen. The
/// bundle path comes from `current_exe()` and can be anywhere the user dragged
/// the app, spaces and apostrophes included.
#[cfg(target_os = "macos")]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ── Uninstall (client role) ─────────────────────────────────────────────────

/// Fully remove the bundled Lucidos install from the GUI — modeled on Docker
/// Desktop's uninstall so a non-developer never needs a terminal. Stops the
/// background service + embedded Postgres, removes the launchd agent + plist,
/// deletes the support data (the embedded database + workspaces AND the WKWebView
/// web storage — localStorage + service worker — only when `delete_data`; the
/// ephemeral caches/prefs/saved-state always), and moves the running `.app` to
/// the Trash. Clearing the WebView storage on `delete_data` is what makes a
/// reinstall start clean (see `support_data_paths` for why a leftover service
/// worker / `lucidos-last-workspace` wedges the next boot on the picker).
///
/// Every step is best-effort + logged and continues on error; failures are
/// collected. Returns `Ok(())` only when the CRITICAL steps succeeded — booting
/// out the service (or it not being loaded) AND every attempted support-data
/// deletion — otherwise `Err` with all collected failures joined. Stopping
/// engines / Postgres, deleting the plist, and trashing the bundle are
/// best-effort: their failures are logged + included in any returned `Err`, but
/// do not on their own fail the uninstall.
///
/// Touches ONLY the bundled install's paths (`com.lucidos.app` / `lucidos-app`
/// under `~/Library`, and the running `.app`) — never the developer dev-setup
/// dirs (`~/projects/lucidos`, `~/workspaces`, `~/.lucidos`).
#[cfg(target_os = "macos")]
pub fn uninstall(app_data: &Path, delete_data: bool) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();

    // (a) Stop per-workspace engines — best-effort + logged internally.
    stop_workspace_engines(app_data);

    // (b) Stop the embedded Postgres cluster BEFORE deleting its data dir, so no
    //     running postmaster holds the tree. Best-effort; logs internally.
    match resource_dir_from_exe() {
        Ok(resources) => stop_embedded_postgres(&resources, app_data),
        Err(e) => {
            eprintln!("[service] uninstall: cannot resolve resources to stop Postgres: {e}")
        }
    }

    // (c) Stop + unload the launchd agent (CRITICAL). `bootout` errors when the
    //     job isn't loaded — that's the goal already met, not a failure — so only
    //     bootout when it is actually loaded.
    let bootout_ok = if is_service_loaded() {
        match bootout_service() {
            Ok(()) => {
                eprintln!("[service] uninstall: launchd service booted out");
                true
            }
            Err(e) => {
                eprintln!("[service] uninstall: bootout failed: {e}");
                failures.push(format!("stop background service: {e}"));
                false
            }
        }
    } else {
        eprintln!("[service] uninstall: launchd service not loaded; nothing to stop");
        true
    };

    // (d) Delete the LaunchAgent plist (best-effort) so it can't reload at login.
    match plist_path() {
        Ok(plist) => match delete_path(&plist) {
            Ok(()) => eprintln!("[service] uninstall: removed {}", plist.display()),
            Err(e) => {
                eprintln!(
                    "[service] uninstall: failed to remove {}: {e}",
                    plist.display()
                );
                failures.push(format!("delete {}: {e}", plist.display()));
            }
        },
        Err(e) => {
            eprintln!("[service] uninstall: cannot resolve plist path: {e}");
            failures.push(format!("resolve plist path: {e}"));
        }
    }

    // (e) Delete the support-data tree (CRITICAL).
    let mut data_ok = true;
    match std::env::var_os("HOME") {
        Some(home) => {
            for path in support_data_paths(Path::new(&home), app_data, delete_data) {
                match delete_path(&path) {
                    Ok(()) => eprintln!("[service] uninstall: removed {}", path.display()),
                    Err(e) => {
                        eprintln!(
                            "[service] uninstall: failed to remove {}: {e}",
                            path.display()
                        );
                        failures.push(format!("delete {}: {e}", path.display()));
                        data_ok = false;
                    }
                }
            }
        }
        None => {
            eprintln!("[service] uninstall: HOME not set; cannot resolve support-data paths");
            failures.push("HOME not set; cannot resolve support-data paths".to_string());
            data_ok = false;
        }
    }

    // (f) Move the running .app bundle to the Trash (best-effort) — derived from
    //     the current exe, so it trashes wherever the app actually lives. Done
    //     LAST so current_exe() stays valid for the resource resolution above.
    match std::env::current_exe()
        .ok()
        .and_then(|exe| app_bundle_root_from_exe(&exe))
    {
        Some(bundle) => match trash_or_remove_bundle(&bundle) {
            Ok(()) => eprintln!(
                "[service] uninstall: moved app bundle {} to Trash",
                bundle.display()
            ),
            Err(e) => {
                eprintln!("[service] uninstall: failed to remove app bundle: {e}");
                failures.push(e);
            }
        },
        None => {
            // Unbundled (e.g. `tauri dev`) — nothing to trash. Not a failure.
            eprintln!(
                "[service] uninstall: not running from a .app bundle; skipping bundle removal"
            );
        }
    }

    if bootout_ok && data_ok {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall(_app_data: &Path, _delete_data: bool) -> Result<(), String> {
    Err("Uninstall is only supported on macOS".to_string())
}

/// The support-data paths the uninstall removes, derived from `$HOME`. The App
/// Support data dir (the embedded Postgres cluster + all workspaces) AND the
/// WKWebView web-storage trees (`~/Library/WebKit/<id>`, `~/Library/HTTPStorages/<id>`)
/// are included ONLY when `delete_data`; the ephemeral caches / preferences /
/// saved window state are always included. Pure path construction (no IO) so it
/// is unit-testable against a fake HOME.
///
/// The WebKit trees are load-bearing for a clean reinstall: they hold the
/// embedded WebView's `localStorage` (the device-global `lucidos-last-workspace`
/// key) and the registered **service worker** + its `CacheStorage`/`IndexedDB`.
/// Without removing them, a "delete my data" uninstall leaves both behind — on
/// reinstall the stale `lucidos-last-workspace` drives the cold-start redirect
/// (index.html) to the now-deleted workspace slug, and the surviving service
/// worker serves its cached `/<slug>/` shell, so the app boots `<App/>` against a
/// workspace the gateway 404s and wedges the boot splash instead of reaching the
/// picker. Both the current (`com.lucidos.app`) and legacy (`lucidos-app`) bundle
/// names are cleared, mirroring the Caches removal. The keep-data path leaves
/// them intact: the workspaces survive, so the SW + last-workspace memory are
/// still valid (and theme/font/scale prefs in `localStorage` are preserved).
#[cfg(target_os = "macos")]
fn support_data_paths(home: &Path, app_data: &Path, delete_data: bool) -> Vec<PathBuf> {
    let library = home.join("Library");
    let mut paths = Vec::new();
    if delete_data {
        // The authoritative data dir (`~/Library/Application Support/<id>`),
        // passed in so it matches exactly what the service uses.
        paths.push(app_data.to_path_buf());
        // The WKWebView web storage (localStorage + service worker +
        // CacheStorage/IndexedDB). Removing it is what makes a reinstall actually
        // clean — see the doc comment above. Both bundle names, like Caches.
        paths.push(library.join("WebKit").join(BUNDLE_IDENTIFIER));
        paths.push(library.join("WebKit").join("lucidos-app"));
        // Disk HTTP cache + cookies (may not exist on every machine; delete_path
        // treats a missing path as success).
        paths.push(library.join("HTTPStorages").join(BUNDLE_IDENTIFIER));
        paths.push(library.join("HTTPStorages").join("lucidos-app"));
        paths.push(
            library
                .join("HTTPStorages")
                .join(format!("{BUNDLE_IDENTIFIER}.binarycookies")),
        );
    }
    paths.push(library.join("Caches").join(BUNDLE_IDENTIFIER));
    paths.push(library.join("Caches").join("lucidos-app"));
    paths.push(
        library
            .join("Preferences")
            .join(format!("{BUNDLE_IDENTIFIER}.plist")),
    );
    paths.push(
        library
            .join("Saved Application State")
            .join(format!("{BUNDLE_IDENTIFIER}.savedState")),
    );
    paths
}

/// Walk an executable path up to its enclosing `.app` bundle root. A macOS
/// bundle runs `<bundle>.app/Contents/MacOS/<exe>`, so the bundle is three
/// parents up. Returns `None` when the exe isn't inside a `.app` (e.g. an
/// unbundled `tauri dev` / `cargo` binary), so the caller skips bundle removal.
#[cfg(target_os = "macos")]
fn app_bundle_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.parent()?.parent()?.parent()?;
    if bundle
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
    {
        Some(bundle.to_path_buf())
    } else {
        None
    }
}

/// Delete a file or directory tree. A missing path is success (the goal — it's
/// already gone). Uses `symlink_metadata` so a symlink is removed as a link, not
/// followed.
#[cfg(target_os = "macos")]
fn delete_path(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Move the app bundle to the user's Trash via Finder (recoverable), falling
/// back to `rm -rf` if the AppleScript path fails.
#[cfg(target_os = "macos")]
fn trash_or_remove_bundle(bundle: &Path) -> Result<(), String> {
    match move_bundle_to_trash(bundle) {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("[service] uninstall: Finder trash failed ({e}); falling back to rm -rf");
            std::fs::remove_dir_all(bundle)
                .map_err(|e2| format!("remove app bundle {}: {e2}", bundle.display()))
        }
    }
}

/// `tell application "Finder" to delete POSIX file "<path>"` — moves the bundle
/// to the Trash so it's recoverable rather than permanently deleted.
#[cfg(target_os = "macos")]
fn move_bundle_to_trash(bundle: &Path) -> Result<(), String> {
    let script = format!(
        r#"tell application "Finder" to delete POSIX file "{}""#,
        applescript_escape(&bundle.to_string_lossy())
    );
    let out = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("run osascript: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Finder delete failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Escape a string for embedding inside an AppleScript double-quoted literal.
#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ── Gateway boot (service role) ─────────────────────────────────────────────

/// Spawn the bundled standalone `lucidos-gateway` (ADR 0014) pointed at the OS
/// app-data dir, configured for the EMBEDDED Postgres backend (bundled
/// `initdb`/`pg_ctl`). The gateway owns the rest: it reads/creates the workspace
/// registry, provisions one embedded shared Postgres cluster with one database
/// per workspace, and spawns each workspace's engine (by path via
/// `LUCIDOS_ENGINE_BIN`, inheriting `LUCIDOS_STATIC_DIR` /
/// `LUCIDOS_SDK_DIR` / `LUCIDOS_SYSTEM_KNOWHOW_DIR` /
/// `LUCIDOS_BOOT_WITHOUT_PROVIDER` from this env), and
/// reverse-proxies `/<slug>/`. First run creates no workspace — the smart root
/// serves the picker so the user names their first one.
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
        // Absolute path to the bundled `lucidos` CLI. The engine reads this in
        // `lucidos_cli_dir()` so the coding-agent permission/question MCP
        // servers, CC hooks, and chat-script `lucidos …` calls resolve without
        // relying on a dev-only PATH (the gateway passes its env to engines).
        .env("LUCIDOS_CLI_BIN", &bundle.cli_bin)
        .env("LUCIDOS_STATIC_DIR", &bundle.frontend)
        .env("LUCIDOS_SDK_DIR", &bundle.sdk)
        // Absolute path to the bundled engine-shipped reference knowhow
        // (`system-knowhow/`). The engine resolves it here (via
        // `resolve_system_knowhow_dir`) so `load_knowhow('system-knowhow/…')`,
        // `GET /api/v1/knowhow`, and the data-API read path all work in a
        // packaged install (the gateway passes its env to engines).
        .env("LUCIDOS_SYSTEM_KNOWHOW_DIR", &bundle.system_knowhow)
        .env("FASTEMBED_CACHE_DIR", &fastembed_cache)
        .env("LUCIDOS_BOOT_WITHOUT_PROVIDER", "1")
        // Tell the gateway it's a packaged build so the picker hides the dev-only
        // gateway self-reload control (packaged updates go through the app updater
        // + a full service restart, not a gateway re-exec). Mirrors the
        // BOOT_WITHOUT_PROVIDER "I am packaged" signal above.
        .env("LUCIDOS_PACKAGED", "1")
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

/// Like [`http_ok`] but returns the response body on a 200 (else `None`). The
/// desktop client deliberately has no reqwest dependency, so this minimal raw
/// HTTP/1.0 GET is reused for the dock-badge poll's small JSON read.
#[cfg(target_os = "macos")]
fn http_get_body(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    let (head, body) = buf.split_once("\r\n\r\n")?;
    let first = head.lines().next().unwrap_or_default();
    if first.contains(" 200 ") || first.ends_with(" 200") {
        Some(body.to_string())
    } else {
        None
    }
}

/// The fresh aggregate unread total across running workspaces — the dock-badge
/// value. `None` (no badge change) when the gateway is unreachable or the body
/// doesn't parse. Reads the gateway's on-demand `unread-total` control endpoint
/// (a live count fan-out over running engines) rather than the cached
/// `last_unread`: at nudge time — right after a read — the supervise loop hasn't
/// re-probed yet, so the cached aggregate would still show the pre-read count.
/// Using the fresh endpoint for BOTH the periodic tick and the nudge also avoids
/// a flicker where a stale tick overwrites a freshly nudged value.
#[cfg(target_os = "macos")]
fn fetch_unread_total(port: u16) -> Option<u64> {
    let body = http_get_body(port, "/~/api/v1/control/unread-total")?;
    parse_unread_total(&body)
}

/// Parse `{ "total": N }` from the `unread-total` endpoint. Pure, so it's
/// unit-tested. `None` when the body doesn't parse or lacks a numeric `total`.
#[cfg(target_os = "macos")]
fn parse_unread_total(body: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json.get("total")?.as_u64()
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

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_unread_total_reads_total_field() {
        assert_eq!(parse_unread_total(r#"{"total":7}"#), Some(7));
        assert_eq!(parse_unread_total(r#"{"total":0}"#), Some(0));
        // Missing / wrong-typed / unparseable → None (no badge change).
        assert_eq!(parse_unread_total(r#"{"workspaces":[]}"#), None);
        assert_eq!(parse_unread_total(r#"{"total":"3"}"#), None);
        assert_eq!(parse_unread_total("not json"), None);
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
        assert_eq!(bundle.cli_bin, resources.join(CLI_RESOURCE_NAME));
        assert_eq!(bundle.frontend, resources.join(FRONTEND_RESOURCE_NAME));
        assert_eq!(bundle.sdk, resources.join(SDK_RESOURCE_NAME));
        assert_eq!(
            bundle.system_knowhow,
            resources.join(SYSTEM_KNOWHOW_RESOURCE_NAME)
        );
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

    #[cfg(target_os = "macos")]
    #[test]
    fn support_data_paths_full_wipe_targets_the_bundled_install_only() {
        // Path construction only — this deletes nothing. A fake HOME proves no
        // username / absolute path is hardcoded.
        let home = Path::new("/fake/home");
        let app_data = Path::new("/fake/home/Library/Application Support/com.lucidos.app");
        let paths = support_data_paths(home, app_data, true);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/fake/home/Library/Application Support/com.lucidos.app"),
                // WKWebView web storage (localStorage + service worker +
                // CacheStorage/IndexedDB) — both bundle names, so a reinstall is clean.
                PathBuf::from("/fake/home/Library/WebKit/com.lucidos.app"),
                PathBuf::from("/fake/home/Library/WebKit/lucidos-app"),
                // Disk HTTP cache + cookies.
                PathBuf::from("/fake/home/Library/HTTPStorages/com.lucidos.app"),
                PathBuf::from("/fake/home/Library/HTTPStorages/lucidos-app"),
                PathBuf::from("/fake/home/Library/HTTPStorages/com.lucidos.app.binarycookies"),
                PathBuf::from("/fake/home/Library/Caches/com.lucidos.app"),
                PathBuf::from("/fake/home/Library/Caches/lucidos-app"),
                PathBuf::from("/fake/home/Library/Preferences/com.lucidos.app.plist"),
                PathBuf::from(
                    "/fake/home/Library/Saved Application State/com.lucidos.app.savedState"
                ),
            ]
        );
        // Never touches the developer dev-setup dirs.
        for p in &paths {
            let s = p.to_string_lossy();
            assert!(!s.contains("/projects/lucidos"), "{s}");
            assert!(!s.contains("/workspaces"), "{s}");
            assert!(!s.contains("/.lucidos"), "{s}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn support_data_paths_keep_data_preserves_the_app_support_dir() {
        let home = Path::new("/fake/home");
        let app_data = Path::new("/fake/home/Library/Application Support/com.lucidos.app");
        let paths = support_data_paths(home, app_data, false);
        // App Support (the database + workspaces) is NOT in the delete set.
        assert!(!paths.contains(&app_data.to_path_buf()));
        // Keep-data preserves the workspaces, so the WebView storage (service
        // worker + last-workspace memory + prefs) must survive with them.
        for p in &paths {
            let s = p.to_string_lossy();
            assert!(!s.contains("/WebKit/"), "{s}");
            assert!(!s.contains("/HTTPStorages/"), "{s}");
        }
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/fake/home/Library/Caches/com.lucidos.app"),
                PathBuf::from("/fake/home/Library/Caches/lucidos-app"),
                PathBuf::from("/fake/home/Library/Preferences/com.lucidos.app.plist"),
                PathBuf::from(
                    "/fake/home/Library/Saved Application State/com.lucidos.app.savedState"
                ),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_bundle_root_from_exe_walks_to_the_dot_app() {
        let exe = Path::new("/Applications/Lucidos.app/Contents/MacOS/Lucidos");
        assert_eq!(
            app_bundle_root_from_exe(exe),
            Some(PathBuf::from("/Applications/Lucidos.app"))
        );
        // Trashes wherever the bundle actually lives, not a hardcoded location.
        let elsewhere = Path::new("/Users/someone/Desktop/Lucidos.app/Contents/MacOS/Lucidos");
        assert_eq!(
            app_bundle_root_from_exe(elsewhere),
            Some(PathBuf::from("/Users/someone/Desktop/Lucidos.app"))
        );
        // An unbundled binary (dev / cargo) has no `.app` root → skip removal.
        assert_eq!(
            app_bundle_root_from_exe(Path::new("/usr/local/bin/lucidos")),
            None
        );
        assert_eq!(app_bundle_root_from_exe(Path::new("/lucidos")), None);
    }

    // ── The relaunch watcher ─────────────────────────────────────────────────

    /// The script for a plain `/Applications` install with no arguments.
    #[cfg(target_os = "macos")]
    fn watcher_script(pid: u32) -> String {
        relaunch_watcher_script(pid, Path::new("/Applications/Lucidos.app"), &[])
            .expect("an ASCII path and no args build a script")
    }

    // The whole point of the watcher: LaunchServices must be asked only once we
    // are gone. Launching while this process is alive would activate the DYING
    // instance instead of starting a new one, which is the bug wearing a
    // different hat.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_waits_for_our_exit_before_it_launches() {
        let script = watcher_script(4242);
        let probe = script.find("kill -0 4242").expect("it probes OUR pid");
        let launch = script.find("/usr/bin/open").expect("it launches the app");
        assert!(
            probe < launch,
            "the wait must precede the launch, got: {script}"
        );
    }

    // The wait is bounded so an orphaned shell cannot loop forever, and the
    // launch sits after the loop rather than inside it.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_stops_waiting_eventually() {
        let script = watcher_script(4242);
        assert!(
            script.contains(&format!("[ $i -lt {RELAUNCH_WAIT_PROBES} ]")),
            "the wait must be bounded, got: {script}"
        );
        let done = script.rfind("done;").expect("the loop ends");
        let launch = script.find("/usr/bin/open").expect("it launches the app");
        assert!(done < launch, "the launch follows the loop: {script}");
    }

    // Reaching the ceiling is NOT a reason to launch. `open` against a client
    // that is still alive would only activate it, and the watcher would then be
    // gone by the time that client actually exited, so the relaunch would be
    // spent on nothing and the app would stay closed.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_launches_only_once_the_client_is_actually_gone() {
        let script = watcher_script(4242);
        let done = script.rfind("done;").expect("the loop ends");
        let guard = script[done..]
            .find("kill -0 4242")
            .map(|i| done + i)
            .expect("the launch is guarded by a final liveness probe");
        let launch = script.find("/usr/bin/open").expect("it launches the app");
        assert!(
            guard < launch,
            "the guard must precede the launch: {script}"
        );
        assert!(
            script[guard..launch].contains("||"),
            "the launch must run only when that probe FAILS: {script}",
        );
    }

    // The bundle path comes from `current_exe()`, so it is wherever the user
    // dragged the app. An unquoted space would launch the wrong thing (or
    // nothing) for anyone whose app is not in /Applications.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_quotes_an_awkward_bundle_path() {
        let bundle = Path::new("/Users/me/My Apps/It's Lucidos.app");
        let script = relaunch_watcher_script(1, bundle, &[]).expect("an awkward path still builds");
        assert!(
            script.contains(r#"open -a '/Users/me/My Apps/It'\''s Lucidos.app'"#),
            "got: {script}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sh_quote_wraps_a_word_and_escapes_embedded_single_quotes() {
        assert_eq!(
            sh_quote("/Applications/Lucidos.app"),
            "'/Applications/Lucidos.app'"
        );
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    // `restart_app` re-execs with the arguments it was launched with, so the
    // LaunchServices path has to carry them too, behind `--args`.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_forwards_arguments_and_omits_the_flag_when_there_are_none() {
        use std::ffi::OsString;

        let bundle = Path::new("/Applications/Lucidos.app");
        let args = vec![OsString::from("--flag"), OsString::from("a value")];
        let with = relaunch_watcher_script(1, bundle, &args).expect("args build");
        assert!(
            with.ends_with(r#"--args '--flag' 'a value'"#),
            "got: {with}"
        );
        assert!(!watcher_script(1).contains("--args"), "no args, no flag");
    }

    // A byte sequence that isn't UTF-8 cannot be quoted into a shell word
    // without corrupting it. Refusing hands the caller back to its own respawn,
    // which passes `OsString`s through faithfully.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_refuses_arguments_it_cannot_quote() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let args = vec![OsString::from_vec(vec![0x2d, 0x2d, 0xff])];
        assert!(
            relaunch_watcher_script(1, Path::new("/Applications/Lucidos.app"), &args).is_err(),
            "a non-UTF-8 argument must not be silently mangled into the script",
        );
    }

    // ── What the next boot owes the user ─────────────────────────────────────

    /// A throwaway app-data dir that removes itself.
    struct TempAppData(PathBuf);

    impl TempAppData {
        fn new(tag: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("lucidos-next-boot-{tag}-{unique}"));
            std::fs::create_dir_all(&path).expect("create the temp app-data dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn record(&self) -> Option<String> {
            std::fs::read_to_string(next_boot_path(&self.0)).ok()
        }

        /// Lay down `workspaces/<id>/.lucidos/engine.pid` holding `pid`.
        fn write_pidfile(&self, id: &str, pid: u32) {
            let dir = self.0.join("workspaces").join(id).join(".lucidos");
            std::fs::create_dir_all(&dir).expect("create the workspace dir");
            std::fs::write(dir.join("engine.pid"), pid.to_string()).expect("write the pidfile");
        }
    }

    impl Drop for TempAppData {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // The record is read by the gateway crate, which shares no code with this
    // one, so the shape can only drift silently. `next_boot.rs`'s
    // `next_boot_record_shape_matches_the_writer` pins the other side.
    #[test]
    fn the_restore_record_is_the_shape_the_gateway_reads() {
        let tmp = TempAppData::new("shape");
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(r#"{"restore":["myws"]}"#));
        assert_eq!(NEXT_BOOT_QUIT, r#"{"quit":true}"#);
        assert_eq!(NEXT_BOOT_FILE, ".next-boot.json");
    }

    // "Quit and Stop Background Service" is the one teardown that means stay
    // down, and it says so before it signals, so the teardown that follows must
    // leave its marker alone rather than recording a restore over it.
    #[test]
    fn a_declared_quit_survives_the_teardown_that_follows_it() {
        let tmp = TempAppData::new("quit");
        declare_quit_intent(tmp.path());
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(NEXT_BOOT_QUIT));
    }

    // A quit intent whose `bootout` failed describes a teardown that never
    // happened, and nothing else clears it (only the gateway's boot consumes the
    // record, and the service is still up). Left behind it would silence the next
    // real restart's restore list.
    #[test]
    fn a_quit_intent_can_be_taken_back_when_the_stop_fails() {
        let tmp = TempAppData::new("unquit");
        declare_quit_intent(tmp.path());
        clear_next_boot_record(tmp.path());
        assert_eq!(tmp.record(), None);
        // And a teardown after that records normally again.
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(r#"{"restore":["myws"]}"#));
    }

    // A workspace may legitimately be called `quit`, and a restore list holding
    // it must not read as a declared stop and silence the next teardown.
    #[test]
    fn a_workspace_named_quit_is_not_a_quit_intent() {
        let tmp = TempAppData::new("named-quit");
        record_workspaces_to_restore(tmp.path(), &["quit".to_string()]);
        assert!(!quit_was_declared(&next_boot_path(tmp.path())));
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(r#"{"restore":["myws"]}"#));
    }

    #[test]
    fn clearing_a_record_that_is_not_there_is_fine() {
        let tmp = TempAppData::new("noclear");
        clear_next_boot_record(tmp.path());
        assert_eq!(tmp.record(), None);
    }

    // Nothing was running, so nothing is owed: a leftover record from an earlier
    // teardown must not resurrect workspaces this one never stopped.
    #[test]
    fn stopping_nothing_clears_any_stale_record() {
        let tmp = TempAppData::new("empty");
        record_workspaces_to_restore(tmp.path(), &["stale".to_string()]);
        record_workspaces_to_restore(tmp.path(), &[]);
        assert_eq!(tmp.record(), None);
    }

    // Liveness, not the pidfile's mere existence, decides what the restart owes:
    // an engine that had already died is not something the user was running.
    #[cfg(unix)]
    #[test]
    fn only_the_engines_that_were_alive_are_recorded() {
        let tmp = TempAppData::new("alive");
        // A stand-in engine: our own child, which SIGUSR1 terminates by default.
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn a stand-in engine");
        tmp.write_pidfile("live-ws", child.id());
        // A pid that cannot exist (macOS caps pids well below this), standing in
        // for a pidfile left behind by an engine that died on its own.
        tmp.write_pidfile("stale-ws", 999_999);

        let stopped = stop_workspace_engines(tmp.path());
        assert_eq!(stopped, vec!["live-ws".to_string()]);
        assert!(
            !tmp.path()
                .join("workspaces/live-ws/.lucidos/engine.pid")
                .exists(),
            "the pidfile is cleared either way",
        );
        let _ = child.wait();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn applescript_escape_escapes_quotes_and_backslashes() {
        assert_eq!(
            applescript_escape(r#"/Apps/Weird "Name"\dir/Lucidos.app"#),
            r#"/Apps/Weird \"Name\"\\dir/Lucidos.app"#
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn delete_path_treats_missing_as_success() {
        let missing = std::env::temp_dir().join(format!("lucidos-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        assert!(delete_path(&missing).is_ok());
    }
}
