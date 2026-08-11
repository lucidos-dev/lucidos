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
//!    It also installs a SECOND agent, the **login agent**
//!    (`~/Library/LaunchAgents/com.lucidos.client.plist`, `RunAtLoad`, one shot),
//!    which `open`s the bundle with [`LOGIN_FLAG`] at login so the client is back
//!    in the menu bar after a restart, menu-bar-only and without a window. Without
//!    it a rebooted Mac runs the service with no client at all, which means no
//!    menu-bar item, no Dock badge and no native notifications (the client is what
//!    shows those).
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

/// Historical launchd label for the **service agent**: the always-on gateway
/// service. The plist installs at
/// `~/Library/LaunchAgents/<SERVICE_AGENT_LABEL>.plist`. The value is historical
/// (`engine`, from before the gateway owned the stack) and must not change: it
/// keys every already-installed plist.
pub const SERVICE_AGENT_LABEL: &str = "com.lucidos.engine";

/// launchd label for the **login agent**: the one-shot job that brings the
/// CLIENT back at login, so the menu-bar item (and with it native
/// notifications, which only the client can show) survives a restart. Distinct
/// from [`SERVICE_AGENT_LABEL`], which is the headless always-on service and
/// hosts no UI at all.
pub const LOGIN_AGENT_LABEL: &str = "com.lucidos.client";

/// The argument the login agent passes the client, marking a launch as
/// "started at login, not by a person". Such a launch comes up menu-bar-only:
/// tray icon, no window, no Dock icon. Read in `lib.rs` by
/// `should_show_window_at_startup`.
///
/// It is launch CONTEXT, not a persistent mode, which is why every relaunch
/// drops it: see [`relaunch_args`].
pub const LOGIN_FLAG: &str = "--login";

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

/// How often the desktop process refreshes the unread indicator (always the
/// menu-bar tray title, plus the dock-icon badge while a window is open) from the
/// gateway's aggregate unread total (macOS only). Independent of the webview's own
/// polling so the count is correct whichever page (picker or a workspace) is loaded.
#[cfg(target_os = "macos")]
const DOCK_BADGE_POLL_INTERVAL: Duration = Duration::from_secs(5);

// ── What the startup splash is told ─────────────────────────────────────────

/// How long a start may take before the splash says anything beyond
/// [`STARTING_LABEL`]. An ordinary launch resolves well inside this, so it never
/// flashes a diagnostic at a user who was not kept waiting.
const STARTUP_QUIET_PERIOD: Duration = Duration::from_secs(8);

/// How long a wait must run before the splash adds the reassurance that a slow
/// start is expected after a restart.
const STARTUP_LONG_WAIT: Duration = Duration::from_secs(60);

/// The splash's opening line, and the one a fast launch shows for its whole
/// life. Mirrored by `main.tsx`'s initial `setBootStatus`, which paints before
/// the first status poll can answer.
const STARTING_LABEL: &str = "Starting Lucidos…";

/// Which part of [`launch`]'s start-and-navigate loop is currently running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPhase {
    /// Installing / kickstarting the launchd service.
    EnsuringService,
    /// Service ensured; polling `/~/api/v1/health` for the gateway.
    WaitingForGateway,
}

/// What [`launch`]'s thread is doing, for the pre-gateway boot splash to read.
///
/// The packaged window paints an inline splash on the bundled asset scheme and
/// cannot reach any API until [`launch`] navigates it to the gateway, so this
/// Tauri-IPC channel is the only thing it can ask. Before it existed the splash
/// showed one static string for however long the wait ran, which is how a start
/// that was recovering on its own read as a hang.
///
/// The sibling of the gateway's `boot_phase` / `boot_failure` narration for the
/// WORKSPACE splash, one layer earlier: same idea, applied to the wait before
/// the gateway answers at all.
pub struct StartupStatus {
    inner: std::sync::Mutex<StartupProgress>,
}

struct StartupProgress {
    phase: StartupPhase,
    /// When the whole start began, NOT when the current phase did: the splash
    /// reports how long the user has been waiting, and that clock must not reset
    /// every time the loop re-ensures the service.
    began: Instant,
    /// The last thing that went wrong, already written as a sentence so it can
    /// be followed by another one. `None` on the ordinary path.
    detail: Option<String>,
}

impl Default for StartupStatus {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(StartupProgress {
                phase: StartupPhase::EnsuringService,
                began: Instant::now(),
                detail: None,
            }),
        }
    }
}

impl StartupStatus {
    /// Move to `phase`. Deliberately does NOT touch `detail`: a failure recorded
    /// while ensuring the service has to survive into the wait that follows it,
    /// which is the only stretch long enough for anyone to read it. Clearing on
    /// every phase change instead made the failure text unreachable, since the
    /// loop moves to `WaitingForGateway` on the line after it records one.
    fn enter(&self, phase: StartupPhase) {
        if let Ok(mut p) = self.inner.lock() {
            p.phase = phase;
        }
    }

    /// Record why the current cycle failed. Pass a sentence.
    fn note_failure(&self, detail: impl Into<String>) {
        if let Ok(mut p) = self.inner.lock() {
            p.detail = Some(detail.into());
        }
    }

    /// Drop any recorded failure, because the thing that failed just worked.
    /// Tied to the outcome rather than to progress through the loop: the wait
    /// after a successful ensure is an ordinary wait and should read as one.
    fn clear_failure(&self) {
        if let Ok(mut p) = self.inner.lock() {
            p.detail = None;
        }
    }

    /// The line to show on the splash right now.
    pub fn label(&self) -> String {
        match self.inner.lock() {
            Ok(p) => startup_label(p.phase, p.began.elapsed(), p.detail.as_deref()),
            // A poisoned lock says nothing useful about the start, and the
            // splash must still say something.
            Err(_) => STARTING_LABEL.to_string(),
        }
    }
}

/// The splash's line for a given phase, elapsed time and last failure. Pure, so
/// the wording is pinned by tests rather than assembled in the poll loop.
///
/// Two rules shape it. A start under [`STARTUP_QUIET_PERIOD`] says exactly what
/// it says today, so nothing changes for the overwhelming majority of launches.
/// Past that, the text names what is being waited on and counts, because a
/// number that moves is what distinguishes "working" from "wedged" to someone
/// looking at a splash screen.
fn startup_label(phase: StartupPhase, elapsed: Duration, detail: Option<&str>) -> String {
    if elapsed < STARTUP_QUIET_PERIOD {
        return STARTING_LABEL.to_string();
    }
    if let Some(detail) = detail {
        return format!("{detail} Retrying…");
    }
    match phase {
        StartupPhase::EnsuringService => "Starting the background service…".to_string(),
        StartupPhase::WaitingForGateway if elapsed >= STARTUP_LONG_WAIT => format!(
            "Waiting for the background service… ({}). It may still be starting up after a restart.",
            humanize_wait(elapsed)
        ),
        StartupPhase::WaitingForGateway => {
            format!("Waiting for the background service… ({})", humanize_wait(elapsed))
        }
    }
}

/// A wait as the splash spells it: `12s`, `1m 05s`. Seconds are zero-padded past
/// the first minute so the line does not change width every tick.
fn humanize_wait(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

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

/// Where [`launch`] should point the main window instead of the gateway root,
/// set at most once by a native notification tap that arrives while the client
/// is still starting.
///
/// A tap has to land in the workspace that raised the banner
/// (`crate::route_native_tap`), but during startup there is nothing to land in:
/// the main window has not been navigated yet and [`launch`] owns its first
/// navigation. Pointing it here ourselves would just be clobbered a moment
/// later. So the tap leaves the destination here and [`launch`] uses it, which
/// is what makes tapping a banner while the client is not running open the
/// client ON that workspace rather than on the picker plus a second window.
static LAUNCH_TARGET: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Aim [`launch`]'s first navigation at `url`. Last writer wins: two taps before
/// the gateway is up can only produce one landing, and the most recent is the
/// one the user just asked for.
///
/// macOS-gated because its one caller is `crate::route_native_tap`, and native
/// banners exist only there. Off macOS nothing sets it and
/// [`take_launch_target`] just keeps answering `None`.
#[cfg(target_os = "macos")]
pub fn set_launch_target(url: String) {
    *LAUNCH_TARGET.lock().unwrap() = Some(url);
}

/// Take the pending launch target, if any. Consuming (rather than peeking) is
/// what keeps it to the FIRST navigation: a tap arriving after the window is
/// live is routed by `crate::route_native_tap` against the real window list
/// instead.
fn take_launch_target() -> Option<String> {
    LAUNCH_TARGET.lock().unwrap().take()
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

    // Unread indicator: mirror the gateway's aggregate unread total (across running
    // workspaces) onto the menu-bar tray title, and onto the Dock badge too while a
    // window is open. Its own thread (independent of the service/health/navigate
    // flow below, and of whichever page the webview shows), so the count always
    // reflects the TOTAL, not just the active workspace. macOS only (both surfaces
    // are macOS concepts). The AppKit write is marshalled to the main thread; the
    // fetch tolerates the gateway not being up yet (returns None until it answers).
    //
    // Event-driven AND polled: the loop recomputes the instant it's NUDGED (the
    // active workspace's webview signals `nudge_dock_badge` when a notification SSE
    // arrives, whether read in-app or from another device) so the count updates without
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
                        let h = handle.clone();
                        let applied = handle.run_on_main_thread(move || {
                            // Always onto the menu-bar tray title, and onto the
                            // Dock badge as well while a window is open (Regular);
                            // menu-bar-only (Accessory) has no Dock tile.
                            crate::apply_unread_indicator(&h, total);
                        });
                        // Record the value only once the hop is ACCEPTED. Nothing
                        // reads the tray or the Dock tile back, so `last` is the
                        // loop's only account of what is on screen: banking a hop
                        // that never queued would pin both surfaces at a stale count
                        // that the changed-value guard then refuses to rewrite. An
                        // unaccepted hop stays unrecorded and the next tick retries.
                        match applied {
                            Ok(()) => last = Some(total),
                            Err(e) => eprintln!("[Tauri] unread indicator not applied: {e}"),
                        }
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
        //
        // Each step also tells `StartupStatus` where it is, which is the only
        // thing the splash on the other side of the IPC bridge can read: waiting
        // silently is what made a recovering start look like a hung one.
        let status = handle.state::<StartupStatus>();
        loop {
            status.enter(StartupPhase::EnsuringService);
            match ensure_service_installed_and_running(&app_data) {
                // Whatever went wrong last cycle is over: this wait is ordinary.
                Ok(()) => status.clear_failure(),
                Err(e) => {
                    eprintln!("[desktop] ensure service running failed: {e}");
                    status.note_failure(format!("Could not start the background service: {e}."));
                }
            }
            status.enter(StartupPhase::WaitingForGateway);
            if wait_for_health(port, HEALTH_ENSURE_CYCLE) {
                break;
            }
            eprintln!(
                "[desktop] gateway not healthy yet on port {port}; re-ensuring service and retrying"
            );
        }

        // A notification tap that arrived while we were waiting names the
        // workspace to land on; otherwise the gateway root (the picker).
        navigate_main_window(
            &handle,
            take_launch_target().unwrap_or_else(|| gateway_url(port)),
        );
        // A tap that landed between the take above and the navigate would
        // otherwise be stranded on a window already pointed elsewhere. Cheap to
        // re-check, and it closes the only window in which the aim can be lost.
        if let Some(url) = take_launch_target() {
            navigate_main_window(&handle, url);
        }
    });
}

/// Point the declared main window at `url`. Best-effort: a missing window or an
/// unparseable URL is logged, never fatal, since the alternative is stranding
/// the user on the bundled "Starting Lucidos…" splash with no explanation.
fn navigate_main_window(app: &AppHandle, url: String) {
    match (
        app.get_webview_window(crate::MAIN_WINDOW_LABEL),
        url.parse::<tauri::Url>(),
    ) {
        (Some(win), Ok(parsed)) => {
            if let Err(e) = win.navigate(parsed) {
                eprintln!("[desktop] failed to navigate window to {url}: {e}");
            }
        }
        _ => eprintln!("[desktop] no main window / bad URL: {url}"),
    }
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

/// How a wait for the freshly-spawned gateway ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayStart {
    /// It answered `/~/api/v1/health`.
    Healthy,
    /// The child process is gone. Nothing will ever answer, so stop waiting.
    ChildExited,
    /// Still alive, still not answering, and out of time.
    TimedOut,
}

/// Decide a single poll of the two conditions [`await_gateway_start`] watches.
/// `None` means neither is decisive yet, so keep waiting.
///
/// **Health is checked first, and the order is load-bearing.** A gateway that
/// answered and only then exited has done its job for this function: the
/// supervise loop below is what notices the exit, and it reacts by shutting down
/// for a launchd respawn, which is a different and correct thing from reporting
/// that the boot failed.
///
/// Pure, so the ordering is pinned by a test rather than by reading the loop.
fn gateway_start_poll(healthy: bool, exited: bool, out_of_time: bool) -> Option<GatewayStart> {
    if healthy {
        return Some(GatewayStart::Healthy);
    }
    if exited {
        return Some(GatewayStart::ChildExited);
    }
    if out_of_time {
        return Some(GatewayStart::TimedOut);
    }
    None
}

/// Wait for a just-spawned gateway to answer, giving up the moment its process
/// is gone rather than serving out the whole deadline.
///
/// The deadline exists for a gateway that is slow but PROGRESSING (migrations
/// and embedding warmup on a fresh workspace can take a while), and it still
/// applies to one. What it must not do is govern a gateway that has already
/// exited: a bind failure kills the process in under a second, and waiting the
/// remaining two minutes on the corpse is time the packaged window spends on its
/// startup splash for no reason at all, before launchd even gets the chance to
/// respawn. Watching the child collapses that to the respawn throttle.
fn await_gateway_start(port: u16, timeout: Duration, gateway: &mut Child) -> GatewayStart {
    let deadline = Instant::now() + timeout;
    loop {
        // A failed `try_wait` means we can no longer tell whether the child is
        // alive; treat that as gone rather than waiting out the deadline on a
        // question we cannot answer.
        let exited = !matches!(gateway.try_wait(), Ok(None));
        if let Some(outcome) = gateway_start_poll(
            http_ok(port, "/~/api/v1/health"),
            exited,
            Instant::now() >= deadline,
        ) {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

// ── Service role: the launchd entry point ───────────────────────────────────

/// Headless launchd entry point (`Lucidos --service`). Boots the standalone
/// gateway on the stable port and supervises it, never touching AppKit/Tauri
/// (so no window, no dock icon). Returns the process exit code:
///  * `0` — graceful stop (SIGTERM from `bootout` / `kickstart -k`), or the
///    gateway exited and launchd's `KeepAlive` should respawn us.
///  * non-zero — boot failed; launchd respawns after `ThrottleInterval`.
pub fn run_service() -> i32 {
    // FIRST, before anything else in the process. launchd hands us an
    // environment the user's shell profile never touched, so the provider keys
    // the engine discovers from env are absent and PATH is the bare
    // `/usr/bin:/bin:/usr/sbin:/sbin`. Everything below this line inherits what
    // we have here: the gateway, every workspace engine, every coding agent.
    //
    // Placed at the top rather than beside `spawn_gateway` because it sets
    // process env, which is only sound while the process is single-threaded.
    // `main` reaches here before any Tauri, AppKit or thread setup, and nothing
    // above this statement changes that. See `shell_env` for the whole story.
    #[cfg(target_os = "macos")]
    crate::shell_env::hydrate_login_shell_env();

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
    match await_gateway_start(port, ENGINE_HEALTH_TIMEOUT, &mut svc.gateway) {
        GatewayStart::Healthy => {}
        GatewayStart::ChildExited => {
            // Say WHICH of the two ways the start failed. The gateway logs its
            // own reason (a held port, an address that does not exist yet) to
            // the same file, immediately above this line, so naming the exit
            // points the reader at it instead of at a timeout that never ran.
            eprintln!(
                "[service] the gateway exited before answering on port {port}; see its error above"
            );
            svc.shutdown(&resources, &app_data);
            return 1;
        }
        GatewayStart::TimedOut => {
            eprintln!(
                "[service] gateway did not become healthy on port {port} within {}s",
                ENGINE_HEALTH_TIMEOUT.as_secs()
            );
            svc.shutdown(&resources, &app_data);
            return 1;
        }
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
/// loaded and running. Installs the login agent alongside it, so the client
/// itself also comes back at the next login.
///
/// Note: the plist captures `current_exe()`. A SIGNED + NOTARIZED app in
/// `/Applications` has a stable path. An UNSIGNED local test build run from
/// Downloads can be Gatekeeper *app-translocated* to a random read-only mount,
/// so the captured path would later vanish — move the `.app` into
/// `/Applications` (or sign it) before relying on the service across reboots.
fn ensure_service_installed_and_running(app_data: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;

    // Best-effort and deliberately first-and-forgotten: the service is what this
    // function must not fail to deliver, and a login agent that did not install
    // costs only what every build before it already cost, a client the user
    // opens by hand.
    #[cfg(target_os = "macos")]
    ensure_login_agent_installed(&exe, app_data);

    let changed = install_or_update_service_plist(&exe, app_data)?;

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

/// `~/Library/LaunchAgents/<label>.plist` for any of our agents.
fn agent_plist_path(label: &str) -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist")))
}

/// The service agent's plist, `~/Library/LaunchAgents/com.lucidos.engine.plist`.
fn service_plist_path() -> io::Result<PathBuf> {
    agent_plist_path(SERVICE_AGENT_LABEL)
}

/// The login agent's plist, `~/Library/LaunchAgents/com.lucidos.client.plist`.
#[cfg(target_os = "macos")]
fn login_plist_path() -> io::Result<PathBuf> {
    agent_plist_path(LOGIN_AGENT_LABEL)
}

/// Minimal XML text escaping for text embedded in a plist `<string>`.
fn xml_escape_str(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Minimal XML text escaping for paths embedded in the plist.
fn xml_escape(p: &Path) -> String {
    xml_escape_str(&p.to_string_lossy())
}

/// The plist that runs `Lucidos --service` at login, restarts it on crash, and
/// logs to the app-data `logs/` dir.
fn desired_service_plist(exe: &Path, app_data: &Path) -> String {
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
        label = SERVICE_AGENT_LABEL,
        exe = xml_escape(exe),
        out = xml_escape(&out),
        err = xml_escape(&err),
    )
}

/// Write `desired` to `path` if it is missing or different, creating the
/// `LaunchAgents` dir and the app-data `logs/` dir the plist points into.
/// Returns true if it was (re)written, which is the caller's cue that launchd is
/// holding a stale definition.
fn write_plist_if_changed(path: &Path, app_data: &Path, desired: &str) -> io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(app_data.join("logs"))?;
    if std::fs::read_to_string(path).ok().as_deref() == Some(desired) {
        return Ok(false);
    }
    std::fs::write(path, desired)?;
    Ok(true)
}

/// Write the service agent's plist if missing or different from desired.
/// Returns true if it was (re)written.
fn install_or_update_service_plist(exe: &Path, app_data: &Path) -> io::Result<bool> {
    let desired = desired_service_plist(exe, app_data);
    write_plist_if_changed(&service_plist_path()?, app_data, &desired)
}

// ── Login agent: the client comes back in the menu bar at login ─────────────

/// How many times the login agent retries `open` before giving up. At login
/// LaunchServices can still be coming up, and the whole point of this agent is
/// that the client is there afterwards, so one refused `open` must not be the
/// end of it.
#[cfg(target_os = "macos")]
const LOGIN_OPEN_ATTEMPTS: u32 = 10;

/// Seconds between those attempts. Bounded rather than `KeepAlive`, so a bundle
/// the user dragged to the Trash cannot leave launchd re-`open`ing it forever.
#[cfg(target_os = "macos")]
const LOGIN_OPEN_RETRY_SECONDS: u32 = 3;

/// The login agent's command: hand the bundle to LaunchServices, in the
/// background, with [`LOGIN_FLAG`], retrying a bounded number of times.
///
/// `open` rather than the bundle's inner binary, for two reasons. It launches
/// the app exactly as a double-click does (the same reason
/// [`relaunch_watcher_script`] uses it), and on an ALREADY-RUNNING client it
/// merely activates that instance, so this job can never produce a second
/// client no matter what kickstarts it. `-g` keeps that activation out of the
/// foreground: a login start belongs in the menu bar, not in front of whatever
/// the user is doing.
///
/// Pure, so the bound, the ordering and the quoting are unit-tested rather than
/// eyeballed. Errors on a bundle path that isn't valid UTF-8: it cannot be
/// quoted into a shell word without corrupting it.
#[cfg(target_os = "macos")]
fn login_launch_script(bundle: &Path) -> Result<String, String> {
    let bundle = bundle
        .to_str()
        .ok_or_else(|| format!("bundle path is not valid UTF-8: {}", bundle.display()))?;
    Ok(format!(
        "i=0; while [ $i -lt {LOGIN_OPEN_ATTEMPTS} ]; do \
         /usr/bin/open -g -a {bundle} --args {LOGIN_FLAG} && exit 0; \
         sleep {LOGIN_OPEN_RETRY_SECONDS}; i=$((i+1)); done; \
         echo \"lucidos: gave up opening {bundle} after {LOGIN_OPEN_ATTEMPTS} attempts; \
         Lucidos will not be in the menu bar until it is opened by hand\" >&2; exit 1",
        bundle = sh_quote(bundle)
    ))
}

/// The plist that brings the CLIENT back at login: one shot, no `KeepAlive`
/// (quitting the client must not respawn it), logging beside the service's own
/// logs.
#[cfg(target_os = "macos")]
fn desired_login_plist(bundle: &Path, app_data: &Path) -> Result<String, String> {
    let script = login_launch_script(bundle)?;
    let logs = app_data.join("logs");
    let out = logs.join("client-login.out.log");
    let err = logs.join("client-login.err.log");
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/sh</string>
        <string>-c</string>
        <string>{script}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
        label = LOGIN_AGENT_LABEL,
        script = xml_escape_str(&script),
        out = xml_escape(&out),
        err = xml_escape(&err),
    ))
}

/// Pure: what the login agent's plist should contain for this executable, or
/// `None` when there is nothing to install because the binary is not inside a
/// `.app` (dev, `cargo run`, an unbundled build) and LaunchServices would have
/// nothing to open. `Err` only for a bundle path that cannot be shell-quoted.
///
/// Split from [`ensure_login_agent_installed`] so the skip-when-unbundled
/// decision is unit-testable without writing to `~/Library/LaunchAgents`.
#[cfg(target_os = "macos")]
fn desired_login_plist_for_exe(exe: &Path, app_data: &Path) -> Result<Option<String>, String> {
    match app_bundle_root_from_exe(exe) {
        Some(bundle) => desired_login_plist(&bundle, app_data).map(Some),
        None => Ok(None),
    }
}

/// Install the login agent, and bootstrap it only when its definition actually
/// changed.
///
/// Two deliberate restraints:
///
///  * **Nothing happens without a `.app`.** A dev or unbundled binary has no
///    bundle for LaunchServices to open, so no plist is written at all.
///  * **An unchanged plist never touches launchd.** Switching the item off in
///    System Settings records a launchd override keyed by the label, which our
///    idempotent write does not clear, and we never `launchctl enable`. So the
///    user's "off" survives every later client launch. A bootstrap is attempted
///    only on a first install or a moved bundle, where launchd genuinely holds
///    nothing or holds a stale path; on a disabled job that attempt fails
///    harmlessly, and the item stays off.
///
/// Every failure is logged and swallowed: the client is already running, so the
/// worst case is the behaviour every build before this one had.
#[cfg(target_os = "macos")]
fn ensure_login_agent_installed(exe: &Path, app_data: &Path) {
    let desired = match desired_login_plist_for_exe(exe, app_data) {
        Ok(Some(desired)) => desired,
        // Dev / unbundled: there is no `.app` for LaunchServices to open.
        Ok(None) => return,
        Err(e) => {
            eprintln!("[desktop] cannot build the login agent plist: {e}");
            return;
        }
    };
    let path = match login_plist_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("[desktop] cannot resolve the login agent plist path: {e}");
            return;
        }
    };
    match write_plist_if_changed(&path, app_data, &desired) {
        Ok(false) => {} // Already installed: leave launchd's view of it alone.
        Ok(true) => {
            let target = login_target();
            if is_job_loaded(&target) {
                // A rewritten definition only takes effect after a reload.
                let _ = bootout_job(&target);
            }
            if let Err(e) = bootstrap_job(&path, &target) {
                eprintln!("[desktop] could not bootstrap the login agent: {e}");
            }
        }
        Err(e) => eprintln!("[desktop] could not install the login agent plist: {e}"),
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// The per-user launchd domain, `gui/<uid>`, which both our agents live in.
fn launchd_domain() -> String {
    format!("gui/{}", current_uid())
}

/// A job's launchd target, `gui/<uid>/<label>`.
fn launchd_target(label: &str) -> String {
    format!("gui/{}/{}", current_uid(), label)
}

/// The service agent's launchd target, `gui/<uid>/com.lucidos.engine`.
fn service_target() -> String {
    launchd_target(SERVICE_AGENT_LABEL)
}

/// The login agent's launchd target, `gui/<uid>/com.lucidos.client`.
#[cfg(target_os = "macos")]
fn login_target() -> String {
    launchd_target(LOGIN_AGENT_LABEL)
}

fn launchctl(args: &[&str]) -> io::Result<std::process::Output> {
    Command::new("launchctl").args(args).output()
}

/// True if `target` is bootstrapped into the user's launchd domain.
fn is_job_loaded(target: &str) -> bool {
    launchctl(&["print", target])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// True if the service agent is bootstrapped into the user's launchd domain.
fn is_service_loaded() -> bool {
    is_job_loaded(&service_target())
}

fn bootstrap_job(plist: &Path, target: &str) -> io::Result<()> {
    let plist_s = plist.to_string_lossy().to_string();
    let out = launchctl(&["bootstrap", &launchd_domain(), &plist_s])?;
    if !out.status.success() {
        // Idempotent re-bootstrap of an already-loaded job is fine; only
        // surface a genuine failure to load.
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !is_job_loaded(target) {
            return Err(io::Error::other(format!(
                "launchctl bootstrap failed: {}",
                stderr.trim()
            )));
        }
    }
    Ok(())
}

fn bootstrap_service() -> io::Result<()> {
    bootstrap_job(&service_plist_path()?, &service_target())
}

fn bootout_job(target: &str) -> io::Result<()> {
    let out = launchctl(&["bootout", target])?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "launchctl bootout failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

fn bootout_service() -> io::Result<()> {
    bootout_job(&service_target())
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

/// This client's argv, minus [`LOGIN_FLAG`], for handing to a relaunch of
/// itself.
///
/// The flag is one-shot launch CONTEXT ("launchd started you at login"), never a
/// mode the process keeps. Both relaunch paths forward argv verbatim, so without
/// this filter a client that came up at login and was later restarted (the
/// updater's relaunch, or the Restart App action) would come back hidden and
/// menu-bar-only even though the user had a window open, which reads as the app
/// vanishing mid-update. It would also quietly undo the frontmost relaunch
/// [`schedule_relaunch_after_exit`] exists to guarantee, since there would be no
/// window to bring forward.
pub fn relaunch_args() -> Vec<std::ffi::OsString> {
    strip_login_flag(std::env::args_os().skip(1))
}

/// Pure half of [`relaunch_args`], so the filter is unit-testable without argv.
fn strip_login_flag(args: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    args.into_iter()
        .filter(|a| a != std::ffi::OsStr::new(LOGIN_FLAG))
        .collect()
}

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
    let args = relaunch_args();
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

    // (c2) Boot out the login agent too (best-effort). It is a one-shot job that
    //      has normally already run and exited, but while it stays loaded a
    //      `kickstart` could still fire it at the bundle we are about to trash.
    let login = login_target();
    if is_job_loaded(&login) {
        match bootout_job(&login) {
            Ok(()) => eprintln!("[service] uninstall: login agent booted out"),
            Err(e) => {
                eprintln!("[service] uninstall: login agent bootout failed: {e}");
                failures.push(format!("stop the login agent: {e}"));
            }
        }
    }

    // (d) Delete BOTH LaunchAgent plists (best-effort) so neither can reload at
    //     login: the service agent, and the login agent that would otherwise
    //     spend a boot trying to `open` a bundle sitting in the Trash.
    for resolved in [service_plist_path(), login_plist_path()] {
        match resolved {
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
    // dir under app-data, and give the gateway a writable CWD. Every engine the
    // gateway spawns INHERITS this, so one ~465 MB copy serves the whole install
    // and uninstalling the app takes it with it.
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

    // ── Waiting for a freshly-spawned gateway ────────────────────────────────

    #[test]
    fn a_dead_gateway_ends_the_wait_immediately() {
        // The reported stall: the gateway died on a bind failure in under a
        // second and the service still waited out its full 120s deadline.
        assert_eq!(
            gateway_start_poll(false, true, false),
            Some(GatewayStart::ChildExited)
        );
    }

    #[test]
    fn a_gateway_that_answered_is_healthy_even_if_it_has_since_exited() {
        // Health is checked first on purpose. An exit after a healthy probe is
        // the supervise loop's business (shut down, let launchd respawn), which
        // is a different outcome from "this boot never came up".
        assert_eq!(
            gateway_start_poll(true, true, true),
            Some(GatewayStart::Healthy)
        );
        assert_eq!(
            gateway_start_poll(true, false, false),
            Some(GatewayStart::Healthy)
        );
    }

    #[test]
    fn a_live_but_silent_gateway_keeps_its_deadline() {
        // Slow-but-progressing is exactly what ENGINE_HEALTH_TIMEOUT is for
        // (migrations + embedding warmup on a fresh workspace). Watching the
        // child must not cut that short.
        assert_eq!(gateway_start_poll(false, false, false), None);
        assert_eq!(
            gateway_start_poll(false, false, true),
            Some(GatewayStart::TimedOut)
        );
    }

    // ── What the startup splash says ─────────────────────────────────────────

    #[test]
    fn a_fast_start_says_exactly_what_it_always_said() {
        // The overwhelming majority of launches resolve inside the quiet period,
        // and none of them should gain a diagnostic they never had.
        for phase in [
            StartupPhase::EnsuringService,
            StartupPhase::WaitingForGateway,
        ] {
            for elapsed in [
                Duration::ZERO,
                STARTUP_QUIET_PERIOD - Duration::from_millis(1),
            ] {
                assert_eq!(startup_label(phase, elapsed, None), STARTING_LABEL);
            }
        }
        // Even a failure stays quiet while the start is still young: the loop
        // re-ensures the service immediately, so a cycle that fails and recovers
        // inside the quiet period never troubles the user with it.
        assert_eq!(
            startup_label(
                StartupPhase::WaitingForGateway,
                Duration::from_secs(1),
                Some("Could not start the background service: nope.")
            ),
            STARTING_LABEL
        );
    }

    #[test]
    fn a_slow_start_names_what_it_is_waiting_for_and_counts() {
        // The point of the whole change: a number that moves is what tells a
        // user staring at a splash that it is working rather than wedged.
        let at_12s = startup_label(
            StartupPhase::WaitingForGateway,
            Duration::from_secs(12),
            None,
        );
        let at_13s = startup_label(
            StartupPhase::WaitingForGateway,
            Duration::from_secs(13),
            None,
        );
        assert_eq!(at_12s, "Waiting for the background service… (12s)");
        assert_ne!(at_12s, at_13s, "the line must change as the wait runs");
        assert_eq!(
            startup_label(StartupPhase::EnsuringService, Duration::from_secs(12), None),
            "Starting the background service…"
        );
    }

    #[test]
    fn a_long_wait_says_a_restart_explains_it() {
        let label = startup_label(
            StartupPhase::WaitingForGateway,
            Duration::from_secs(95),
            None,
        );
        assert_eq!(
            label,
            "Waiting for the background service… (1m 35s). It may still be starting up after a \
             restart."
        );
        // The boundary belongs to the long form, not the short one.
        assert!(
            startup_label(StartupPhase::WaitingForGateway, STARTUP_LONG_WAIT, None)
                .contains("after a restart")
        );
        assert!(!startup_label(
            StartupPhase::WaitingForGateway,
            STARTUP_LONG_WAIT - Duration::from_secs(1),
            None
        )
        .contains("after a restart"));
    }

    #[test]
    fn a_recorded_failure_is_shown_with_the_promise_of_a_retry() {
        // The loop genuinely does retry forever, so the line must not read as a
        // dead end. Same distinction the gateway's BootFailure draws.
        assert_eq!(
            startup_label(
                StartupPhase::WaitingForGateway,
                Duration::from_secs(30),
                Some("Could not start the background service: launchctl bootstrap failed.")
            ),
            "Could not start the background service: launchctl bootstrap failed. Retrying…"
        );
    }

    #[test]
    fn a_wait_is_spelled_without_the_line_changing_width_every_tick() {
        assert_eq!(humanize_wait(Duration::from_secs(0)), "0s");
        assert_eq!(humanize_wait(Duration::from_secs(59)), "59s");
        assert_eq!(humanize_wait(Duration::from_secs(60)), "1m 00s");
        assert_eq!(humanize_wait(Duration::from_secs(69)), "1m 09s");
        assert_eq!(humanize_wait(Duration::from_secs(3599)), "59m 59s");
        assert_eq!(humanize_wait(Duration::from_secs(3600)), "60m 00s");
    }

    /// Wind the start's clock back, so a test can ask what the splash would say
    /// after `elapsed` without sleeping for it.
    fn aged(status: &StartupStatus, elapsed: Duration) {
        if let Ok(mut p) = status.inner.lock() {
            p.began = Instant::now() - elapsed;
        }
    }

    #[test]
    fn the_status_starts_quiet() {
        let status = StartupStatus::default();
        assert_eq!(status.label(), STARTING_LABEL, "a fresh start is quiet");
    }

    #[test]
    fn a_failure_recorded_while_ensuring_survives_into_the_wait() {
        // The bug this pins: the loop records an ensure failure and moves to
        // WaitingForGateway on the very next line, so a phase change that
        // cleared the detail made the failure text unreachable. The wait is the
        // only stretch long enough for anyone to read it.
        let status = StartupStatus::default();
        status.enter(StartupPhase::EnsuringService);
        status.note_failure("Could not start the background service: nope.");
        status.enter(StartupPhase::WaitingForGateway);
        aged(&status, Duration::from_secs(20));

        assert_eq!(
            status.label(),
            "Could not start the background service: nope. Retrying…"
        );
    }

    #[test]
    fn a_cycle_that_ensures_cleanly_drops_the_previous_complaint() {
        // The other half: the detail's life is tied to the OUTCOME, not to
        // progress through the loop, so a service that started on the retry
        // leaves the splash reading as an ordinary wait.
        let status = StartupStatus::default();
        status.note_failure("Could not start the background service: nope.");
        status.enter(StartupPhase::EnsuringService);
        status.clear_failure();
        status.enter(StartupPhase::WaitingForGateway);
        aged(&status, Duration::from_secs(20));

        assert_eq!(status.label(), "Waiting for the background service… (20s)");
    }

    #[test]
    fn xml_escape_escapes_markup_chars() {
        let p = Path::new("/Users/a&b/<x>/Lucidos");
        assert_eq!(xml_escape(p), "/Users/a&amp;b/&lt;x&gt;/Lucidos");
    }

    #[test]
    fn desired_plist_runs_service_role_with_keepalive() {
        let exe = Path::new("/Applications/Lucidos.app/Contents/MacOS/Lucidos");
        let app_data = Path::new("/Users/me/Library/Application Support/com.lucidos.app");
        let plist = desired_service_plist(exe, app_data);

        assert!(plist.contains(&format!("<string>{SERVICE_AGENT_LABEL}</string>")));
        assert!(plist.contains("<string>/Applications/Lucidos.app/Contents/MacOS/Lucidos</string>"));
        assert!(plist.contains("<string>--service</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        // Logs land under the app-data dir, not next to the bundle.
        assert!(plist.contains("Application Support/com.lucidos.app/logs/engine-service.out.log"));
    }

    // ── The login agent ──────────────────────────────────────────────────────

    #[cfg(target_os = "macos")]
    #[test]
    fn login_launch_script_opens_the_bundle_through_launch_services() {
        let script = login_launch_script(Path::new("/Applications/Lucidos.app")).unwrap();

        // LaunchServices, not the inner binary: that is what makes a second
        // client impossible, because `open` activates a running one instead.
        assert!(script.contains("/usr/bin/open -g -a '/Applications/Lucidos.app'"));
        assert!(!script.contains("Contents/MacOS"));
        // And the flag that keeps the login start out of the way.
        assert!(script.contains(&format!("--args {LOGIN_FLAG}")));
        // Backgrounded: a login start belongs in the menu bar, not in front of
        // whatever the user is doing.
        assert!(script.contains(" -g "));
        // Bounded retry, then give up: LaunchServices may not be ready the
        // instant launchd fires, but a trashed bundle must not loop forever.
        assert!(script.contains(&format!("[ $i -lt {LOGIN_OPEN_ATTEMPTS} ]")));
        assert!(script.contains(&format!("sleep {LOGIN_OPEN_RETRY_SECONDS}")));
        assert!(script.trim_end().ends_with("exit 1"));
        // The retry only happens when `open` FAILED; a success leaves at once.
        assert!(script.contains("&& exit 0"));
        // Giving up says so in the agent's own log. "Lucidos is not in my menu
        // bar" is the whole symptom this feature exists to fix, so the one place
        // that can explain it must not exit silently.
        assert!(script.contains("gave up opening"));
        assert!(script.contains(">&2"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn login_launch_script_quotes_a_bundle_path_with_a_quote_in_it() {
        let script =
            login_launch_script(Path::new("/Users/me/don't-quote-me/Lucidos.app")).unwrap();
        assert!(script.contains(r"-a '/Users/me/don'\''t-quote-me/Lucidos.app'"));
        // And the result is still a script `/bin/sh -c` can parse. A quoting slip
        // here is a syntax error at login, which reads as "Lucidos did not start".
        assert_eq!(sh_parses(&script), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_login_script_parses_as_a_shell_script() {
        let script = login_launch_script(Path::new("/Applications/Lucidos.app")).unwrap();
        assert_eq!(sh_parses(&script), Ok(()));
        // Guard the guard: `sh -n` really does reject a broken script, so a
        // passing assertion above means something.
        assert!(sh_parses("while [ 1 ]; do echo").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn desired_login_plist_is_a_one_shot_that_runs_at_login() {
        let bundle = Path::new("/Applications/Lucidos.app");
        let app_data = Path::new("/Users/me/Library/Application Support/com.lucidos.app");
        let plist = desired_login_plist(bundle, app_data).unwrap();

        assert!(plist.contains(&format!("<string>{LOGIN_AGENT_LABEL}</string>")));
        assert!(plist.contains("<string>/bin/sh</string>"));
        assert!(plist.contains("<string>-c</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        // NO KeepAlive: quitting the client must not respawn it, and the job is
        // one shot that exits as soon as `open` has been handed the bundle.
        assert!(!plist.contains("<key>KeepAlive</key>"));
        // The shell `&&` has to survive as XML, or launchd reads a truncated
        // command and the client never starts.
        assert!(plist.contains("&amp;&amp; exit 0"));
        assert!(!plist.contains("&& exit 0"));
        // Its own logs, beside the service's.
        assert!(plist.contains("Application Support/com.lucidos.app/logs/client-login.out.log"));
        assert!(plist.contains("Application Support/com.lucidos.app/logs/client-login.err.log"));
    }

    /// Pipe `input` to `program args…` and report what it said if it refused.
    #[cfg(target_os = "macos")]
    fn check_with(program: &str, args: &[&str], input: &str) -> Result<(), String> {
        use std::io::Write as _;
        use std::process::Stdio;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {program}: {e}"))?;
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(input.as_bytes())
            .map_err(|e| format!("write to {program}: {e}"))?;
        let out = child.wait_with_output().map_err(|e| format!("{e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Err(said.trim().to_string())
    }

    /// Feed a plist to the system parser the way launchd will read it.
    #[cfg(target_os = "macos")]
    fn plutil_lint(plist: &str) -> Result<(), String> {
        check_with("plutil", &["-lint", "-"], plist)
    }

    /// Parse a script the way `/bin/sh -c` will, without running it.
    #[cfg(target_os = "macos")]
    fn sh_parses(script: &str) -> Result<(), String> {
        check_with("/bin/sh", &["-n"], script)
    }

    /// Both plists have to survive the system parser, not just look right. A
    /// malformed one is refused by launchd wholesale, and for the login agent
    /// that failure mode IS the bug it exists to fix: a Mac that comes back from
    /// a restart with no client. The `&&` in the login script is the live
    /// hazard, since raw it is an unknown ampersand-escape.
    #[cfg(target_os = "macos")]
    #[test]
    fn both_agent_plists_parse_as_plists() {
        let app_data = Path::new("/Users/me/Library/Application Support/com.lucidos.app");
        let service = desired_service_plist(
            Path::new("/Applications/Lucidos.app/Contents/MacOS/lucidos-app"),
            app_data,
        );
        assert_eq!(plutil_lint(&service), Ok(()));

        let login = desired_login_plist(Path::new("/Applications/Lucidos.app"), app_data).unwrap();
        assert_eq!(plutil_lint(&login), Ok(()));

        // A path carrying markup characters must not break out of the `<string>`.
        let awkward =
            desired_login_plist(Path::new("/Users/me/App & <Co>/Lucidos.app"), app_data).unwrap();
        assert_eq!(plutil_lint(&awkward), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn no_login_agent_is_installed_for_an_unbundled_binary() {
        let app_data = Path::new("/Users/me/Library/Application Support/com.lucidos.app");
        // Dev / `cargo run`: nothing for LaunchServices to open, so nothing is
        // written, and a dev machine never grows a com.lucidos.client.plist.
        assert_eq!(
            desired_login_plist_for_exe(Path::new("/usr/local/bin/lucidos-app"), app_data),
            Ok(None)
        );
        // A real bundle: the plist names the BUNDLE, never the inner binary.
        let exe = Path::new("/Applications/Lucidos.app/Contents/MacOS/lucidos-app");
        let plist = desired_login_plist_for_exe(exe, app_data)
            .unwrap()
            .expect("a bundled exe installs a login agent");
        assert!(plist.contains("-a '/Applications/Lucidos.app'"));
    }

    #[test]
    fn a_relaunch_never_carries_the_login_flag_forward() {
        let os = |s: &str| std::ffi::OsString::from(s);

        // The bug this prevents: a client the login agent started keeps `--login`
        // in its argv forever, and both relaunch paths forward argv verbatim. So
        // an update or a Restart App would bring the client back HIDDEN, even
        // with a window open at the time, which reads as the app vanishing.
        assert_eq!(
            strip_login_flag([os(LOGIN_FLAG)]),
            Vec::<std::ffi::OsString>::new()
        );
        assert_eq!(
            strip_login_flag([os("--first"), os(LOGIN_FLAG), os("--last")]),
            vec![os("--first"), os("--last")]
        );
        // Everything else is passed through untouched, including near-misses.
        assert_eq!(
            strip_login_flag([os("--login-shell"), os("login")]),
            vec![os("--login-shell"), os("login")]
        );
        assert_eq!(strip_login_flag([]), Vec::<std::ffi::OsString>::new());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_two_agents_are_separate_jobs() {
        // Same domain, distinct labels, distinct plists: installing or booting
        // out one must never reach the other.
        assert_ne!(SERVICE_AGENT_LABEL, LOGIN_AGENT_LABEL);
        assert_ne!(service_target(), login_target());
        assert!(service_target().starts_with(&launchd_domain()));
        assert!(login_target().starts_with(&launchd_domain()));
        assert_eq!(
            agent_plist_path(LOGIN_AGENT_LABEL).ok(),
            login_plist_path().ok()
        );
        assert_ne!(service_plist_path().ok(), login_plist_path().ok());
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
