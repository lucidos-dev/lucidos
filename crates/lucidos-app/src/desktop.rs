//! Always-on desktop runtime for a PACKAGED Lucidos build. Full picture:
//! [`docs/desktop-app.md`](../../../docs/desktop-app.md).
//!
//! Two roles share this one bundled binary, and both are launchd agents.
//!
//!  * **Service** (`Lucidos --service`): [`run_service`] spawns and supervises
//!    the standalone workspace gateway (ADR 0014) on a STABLE port. No window,
//!    no AppKit.
//!  * **Client** (the GUI app): [`launch`] ensures the service is running,
//!    waits for its health, then points the window at it. The login agent
//!    reopens it with [`LOGIN_FLAG`] after a restart, so a rebooted Mac keeps
//!    its menu-bar item and native notifications.
//!
//! **Closing the UI does NOT stop the service.** Only `launchctl bootout`,
//! behind "Quit and Stop Background Service", tears it down. [`launch`]
//! short-circuits on `tauri::is_dev()`, so none of this runs in development.
//!
//! Bundle paths resolve relative to the executable, so the service, which has
//! no `AppHandle`, reaches what the client's `resource_dir()` would. State
//! lives under the OS app-data dir, which the updater never replaces.

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

/// launchd label for the **service agent**, the always-on gateway service. The
/// value is historical, from before the gateway owned the stack, and must not
/// change: it keys every already-installed plist.
pub const SERVICE_AGENT_LABEL: &str = "com.lucidos.engine";

/// launchd label for the **login agent**, the one-shot job that reopens the
/// CLIENT at login. That is what keeps the menu-bar item, and with it native
/// notifications, across a restart. The service agent hosts no UI at all.
pub const LOGIN_AGENT_LABEL: &str = "com.lucidos.client";

/// The argument the login agent passes the client, marking a launch as
/// "started at login, not by a person". Such a launch comes up menu-bar-only:
/// tray icon, no window, no Dock icon. Read in `lib.rs` by
/// `should_show_window_at_startup`.
///
/// It is launch CONTEXT, not a persistent mode, which is why every relaunch
/// drops it: see [`relaunch_args`].
pub const LOGIN_FLAG: &str = "--login";

/// Fixed default port for the gateway, so the mobile connect URL is stable
/// across restarts. The `engine` in the name is historical: under ADR 0014 the
/// gateway owns this public port and each spawned engine binds loopback only.
/// Override with `LUCIDOS_ENGINE_PORT` or `<app-data>/config/engine-port`.
pub const DEFAULT_ENGINE_PORT: u16 = 5252;

/// How long to wait for the gateway to answer `/~/api/v1/health`
/// (migrations + embedding-model warmup can be slow on a fresh workspace).
const ENGINE_HEALTH_TIMEOUT: Duration = Duration::from_secs(120);

/// One health-poll cycle in the client's start-and-navigate loop ([`launch`]):
/// how long to wait for `/~/api/v1/health` before re-ensuring the service and
/// polling again. The loop NEVER gives up, so this only bounds how often a
/// crashed/idle service is re-kickstarted while the window waits.
const HEALTH_ENSURE_CYCLE: Duration = Duration::from_secs(30);

/// How often the desktop process refreshes the unread indicator from the
/// gateway's aggregate total. Independent of the webview's own polling, so the
/// count is right whichever page is loaded.
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

// ── The service's error log, and what the client reads out of it ────────────

/// The service's stderr, under `<app-data>/logs/`. ONE constant, because the
/// plist writes this file and [`launch`] reads it back. Two literals would let
/// the reader drift onto a file nothing writes, and the drift would look
/// exactly like a service that never failed.
const SERVICE_ERR_LOG: &str = "engine-service.err.log";

/// The line [`run_service`] writes before it does anything else, once per
/// launchd start. Counting these is how the client tells a crash loop from a
/// slow boot: see [`parse_service_boots`].
const SERVICE_BOOT_MARKER: &str = "[service] boot starting";

/// The token every fatal boot line carries, from either producer. The service
/// writes `[service] boot failed: …`; the gateway writes
/// `[gateway] boot failed: …` from its own `main`, into this same file. The
/// client greps for the shared token rather than for a producer.
const BOOT_FAILED_MARKER: &str = "boot failed:";

/// How many service starts inside one client wait mean a crash loop rather than
/// a slow boot. Three is not a guess. A slow boot writes exactly ONE marker for
/// at least [`ENGINE_HEALTH_TIMEOUT`], because that deadline is what makes the
/// service exit. Reaching three therefore takes two service exits, which a
/// boot that is merely slow cannot produce inside one wait.
const SERVICE_CRASH_LOOP_BOOTS: usize = 3;

/// How often the wait re-reads the error log. Frequent enough that the report
/// lands within seconds of the failure that earned it, and cheap: a crash loop
/// appends a few hundred bytes per cycle.
const SERVICE_LOG_POLL: Duration = Duration::from_secs(2);

/// How much of the log to read back. A crash cycle costs a few hundred bytes,
/// so this holds thousands of them. The cap exists so a log that grew for other
/// reasons cannot be pulled into memory whole.
const SERVICE_LOG_MAX_BYTES: u64 = 512 * 1024;

/// How much of a reason the splash shows. Long enough for a path-bearing
/// message, short enough that the splash stays a splash.
const MAX_REASON_CHARS: usize = 240;

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
/// The packaged window paints an inline splash off the bundled assets and can
/// reach no API until [`launch`] navigates it to the gateway. This Tauri-IPC
/// channel is the only thing it can ask, which is why a recovering start would
/// otherwise read as a hang.
///
/// The sibling of the gateway's `boot_phase` narration for the WORKSPACE
/// splash, one layer earlier.
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
    /// Set once the service has been seen to restart [`SERVICE_CRASH_LOOP_BOOTS`]
    /// times inside this wait. `None` on the ordinary path, and on a slow one.
    crash: Option<CrashLoop>,
}

/// A background service launchd keeps respawning, as the splash needs to
/// describe it: how many starts, and the reason the last failed one gave.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CrashLoop {
    boots: usize,
    reason: Option<String>,
    log: PathBuf,
}

impl Default for StartupStatus {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(StartupProgress {
                phase: StartupPhase::EnsuringService,
                began: Instant::now(),
                detail: None,
                crash: None,
            }),
        }
    }
}

impl StartupStatus {
    /// Move to `phase`. Deliberately does NOT touch `detail`: the loop moves to
    /// `WaitingForGateway` on the line after it records a failure. That wait is
    /// the only stretch long enough for anyone to read one.
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

    /// Record what the service's error log now says. Below the threshold this
    /// CLEARS the crash report rather than leaving a stale one: a service that
    /// came up after two bad starts leaves the splash reading as a wait again.
    fn note_service_boots(&self, report: &ServiceBootReport, log: &Path) {
        let crash = (report.boots >= SERVICE_CRASH_LOOP_BOOTS).then(|| CrashLoop {
            boots: report.boots,
            reason: report.reason.clone(),
            log: log.to_path_buf(),
        });
        if let Ok(mut p) = self.inner.lock() {
            p.crash = crash;
        }
    }

    /// The line to show on the splash right now.
    pub fn label(&self) -> String {
        match self.inner.lock() {
            Ok(p) => startup_label(
                p.phase,
                p.began.elapsed(),
                p.detail.as_deref(),
                p.crash.as_ref(),
            ),
            // A poisoned lock says nothing useful about the start, and the
            // splash must still say something.
            Err(_) => STARTING_LABEL.to_string(),
        }
    }
}

/// The splash's line for a given phase, elapsed time and last failure. Pure, so
/// the wording is pinned by tests rather than assembled in the poll loop.
///
/// Three rules shape it. A start under [`STARTUP_QUIET_PERIOD`] says only
/// [`STARTING_LABEL`], so the overwhelming majority of launches gain no
/// diagnostic. A crash loop then wins over everything else, because a counter
/// that will never resolve is worse than no counter. Otherwise it names what is
/// being waited on and counts. A number that moves is what tells a waiting user
/// it is working, not wedged.
fn startup_label(
    phase: StartupPhase,
    elapsed: Duration,
    detail: Option<&str>,
    crash: Option<&CrashLoop>,
) -> String {
    if elapsed < STARTUP_QUIET_PERIOD {
        return STARTING_LABEL.to_string();
    }
    if let Some(crash) = crash {
        return crash_loop_label(crash);
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

/// What the splash says about a service launchd keeps respawning. Three lines,
/// mirroring what the headless installer prints on the same condition
/// (`install.sh`, the `LUCIDOS_HEALTH_TIMEOUT` arm): what happened, why, and
/// where to read more.
///
/// It carries NO elapsed counter, which is the point. It also promises no
/// resolution and declares no dead end: the loop does keep retrying, and a
/// reinstall or a repaired bundle still recovers the window.
fn crash_loop_label(crash: &CrashLoop) -> String {
    // STARTS, not failures, because starts are what the log counts. The newest
    // one is still in flight when this is read, so "it failed N times" would
    // claim one failure that has not happened.
    let mut lines = vec![format!(
        "The background service is not starting. It has started {} times without coming up.",
        crash.boots
    )];
    if let Some(reason) = &crash.reason {
        lines.push(truncate_reason(reason));
    }
    lines.push(format!(
        "Lucidos keeps trying. Log: {}",
        crash.log.display()
    ));
    lines.join("\n")
}

/// A reason cut to [`MAX_REASON_CHARS`], on a character boundary. A staging
/// failure names a path, and a path can be arbitrarily long.
fn truncate_reason(reason: &str) -> String {
    match reason.char_indices().nth(MAX_REASON_CHARS) {
        Some((cut, _)) => format!("{}…", &reason[..cut]),
        None => reason.to_string(),
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

/// What the service's error log says about the boots since the client started
/// waiting.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ServiceBootReport {
    /// How many times the service process has started.
    boots: usize,
    /// What the last FAILED start gave as its reason.
    reason: Option<String>,
}

/// Read the service's error log from where the client found it.
///
/// The offset is taken once, when the wait begins, and never advanced. The
/// report is cumulative ("it has failed N times since you started waiting"), so
/// every read covers the same window. Taking the offset at all is what keeps a
/// previous session's crashes out of this session's count.
struct ServiceLogTail {
    path: PathBuf,
    from: u64,
}

impl ServiceLogTail {
    /// Start watching `path` from its current end. A missing file is the
    /// ordinary first-run case and starts at zero.
    fn start(path: &Path) -> Self {
        let from = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Self {
            path: path.to_path_buf(),
            from,
        }
    }

    /// Re-read the window and parse it. An unreadable log yields an empty
    /// report, which reads as "no evidence of a crash loop" and leaves the
    /// ordinary wait label in place.
    fn read(&self) -> ServiceBootReport {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return ServiceBootReport::default();
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        // A log that shrank was rotated or deleted under us, so the recorded
        // offset points past the end. Read the whole of what is there instead
        // of nothing.
        let from = if len < self.from { 0 } else { self.from };
        if std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(from)).is_err() {
            return ServiceBootReport::default();
        }
        // Bytes, then a LOSSY decode. `read_to_string` refuses the whole read on
        // any invalid UTF-8, and two ordinary things produce some: the byte cap
        // can land mid-character, and every child the service spawns shares this
        // stderr. Either would silently turn the crash-loop report off, which is
        // the failure this whole path exists to end.
        let mut window = Vec::new();
        if std::io::Read::by_ref(&mut file)
            .take(SERVICE_LOG_MAX_BYTES)
            .read_to_end(&mut window)
            .is_err()
        {
            return ServiceBootReport::default();
        }
        parse_service_boots(&String::from_utf8_lossy(&window))
    }
}

/// Count the service's starts in `window`, and pull the reason out of the last
/// one that failed. Pure, so the whole contract with the log is unit-tested.
///
/// The reason is the FIRST [`BOOT_FAILED_MARKER`] line of that start, not the
/// last. Both producers write into this one file. The gateway dies before the
/// service notices, so the first line is the gateway's precise, path-bearing
/// reason, and anything after it summarises the same event.
fn parse_service_boots(window: &str) -> ServiceBootReport {
    let mut boots = 0usize;
    // The reason of the start being read. Reset at each marker, so a start that
    // recovered does not inherit the previous start's complaint.
    let mut current: Option<String> = None;
    let mut last_failed: Option<String> = None;

    for line in window.lines() {
        if line.contains(SERVICE_BOOT_MARKER) {
            boots += 1;
            current = None;
            continue;
        }
        // Before the first marker is the tail of a start that began before this
        // wait did. It is not ours to report.
        if boots == 0 || current.is_some() {
            continue;
        }
        if let Some((_, reason)) = line.split_once(BOOT_FAILED_MARKER) {
            let reason = reason.trim();
            if !reason.is_empty() {
                current = Some(reason.to_string());
                last_failed = current.clone();
            }
        }
    }
    ServiceBootReport {
        boots,
        reason: last_failed,
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
/// 0014). The gateway provisions the embedded Postgres and spawns one engine
/// per registered workspace, so this role manages neither directly.
struct GatewayService {
    gateway: Child,
}

impl GatewayService {
    /// Stop the gateway, then the engines it spawned, then the embedded Postgres
    /// cluster. Best-effort, and logs failures.
    ///
    /// The PERMANENT-stop teardown: "Quit and Stop Background Service" and the
    /// supervised-exit path both route here. The gateway's in-place reload does
    /// NOT. It leaves the cluster running for the re-exec'd image to re-adopt,
    /// and stopping Postgres here would break that.
    fn shutdown(&mut self, resources: &Path, app_data: &Path) {
        // SIGUSR1 is the gateway's graceful-stop signal, since it ignores
        // SIGTERM. It exits but deliberately LEAVES its engines running for
        // re-adoption, so we stop those explicitly below.
        //
        // Signalled through `libc::kill`, as `stop_workspace_engines` does.
        // Forking `kill` would put a bare name on the PATH launchd hands this
        // service, for a call the C library already makes directly.
        #[cfg(unix)]
        {
            // SAFETY: signalling our own still-running child; a pid that has
            // already exited returns ESRCH, which is nothing to act on here.
            unsafe {
                libc::kill(self.gateway.id() as libc::pid_t, libc::SIGUSR1);
            }
            for _ in 0..30 {
                match self.gateway.try_wait() {
                    Ok(Some(_)) => break,
                    _ => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
        let _ = self.gateway.kill();
        let _ = self.gateway.wait();
        // A restart and a crash respawn run this same teardown, so record what
        // is being stopped for the next boot. See `record_workspaces_to_restore`.
        let stopped = stop_workspace_engines(app_data);
        record_workspaces_to_restore(app_data, &stopped);
        // Last, after the engines that connect to it. A permanent shutdown must
        // never leave an orphaned `postgres` holding the port and its
        // postmaster.pid for the next app version to trip over.
        stop_embedded_postgres(resources, app_data);
    }
}

/// Stop the embedded Postgres cluster cleanly on a permanent service shutdown.
/// Best-effort and logged, and a no-op when no cluster has been provisioned.
/// Shells out to the bundled `pg_ctl`, because `lucidos-app` links neither the
/// gateway nor the engine crate.
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

/// SIGUSR1 every workspace engine the gateway spawned, returning the ids of the
/// ones that were actually alive. Used on a full service stop. The gateway
/// leaves engines running on its own SIGUSR1 so a restart can re-adopt them,
/// but an explicit stop tears the whole stack down.
///
/// This checks liveness rather than trusting the pidfile, because the returned
/// ids are what the next boot owes the user ([`record_workspaces_to_restore`]).
/// A stale pidfile would otherwise make a restart "restore" a workspace nobody
/// was running.
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
/// back. The reader is `next_boot.rs` in the gateway crate, which this crate
/// does not link. Both spell the same filename and the same JSON, and each
/// pins it with a test.
const NEXT_BOOT_FILE: &str = ".next-boot.json";

fn next_boot_path(app_data: &Path) -> PathBuf {
    app_data.join(NEXT_BOOT_FILE)
}

/// The `{"quit": true}` body: the last teardown was deliberate, restore nothing.
const NEXT_BOOT_QUIT: &str = "{\"quit\":true}";

/// Note down the workspaces the teardown just stopped, so the next gateway boot
/// starts them again.
///
/// A restart must return what it took. `launchctl kickstart -k` and a crash
/// respawn both run the same teardown as a permanent stop. The gateway that
/// comes up afterwards re-adopts only engines that survived, and there are
/// none. So without this the workspace the user was sitting in stays stopped,
/// and its open page cannot wake it: API traffic never lazy-starts a workspace,
/// which is what makes the picker's Stop button stick.
///
/// Skipped when the record already says `quit`. [`stop_service`] writes that
/// BEFORE it signals launchd, so a deliberate quit stays quiet.
///
/// Best-effort. Failing to write it costs a restart its workspaces, and must
/// never take the teardown down with it.
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
/// Only the gateway's boot normally consumes it. An intent whose teardown fell
/// through has to be taken back here, or it silences the next real one.
fn clear_next_boot_record(app_data: &Path) {
    match std::fs::remove_file(next_boot_path(app_data)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("[service] could not clear the next-boot record: {e}"),
    }
}

// ── Path resolution (shared by both roles) ──────────────────────────────────

/// Resolve the bundle's `Resources` dir relative to the executable, so the
/// service resolves the same bundle the client's `resource_dir()` returns.
///
/// Tauri names the bundle from `productName` but the binary inside it from the
/// crate, so `Lucidos.app` holds `Contents/MacOS/lucidos-app`.
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

/// Resolve the stable gateway port: the `LUCIDOS_ENGINE_PORT` override wins,
/// then the persisted `<app-data>/config/engine-port`, else the default. First
/// run writes the default there, so the port stays stable and user-editable.
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

/// The URL a packaged client window is pointed at: the always-on gateway on
/// this install's stable port.
///
/// The ONE place this string is built. The window's resulting origin has to
/// match [`gateway_capability`]'s URL pattern exactly, and every IPC call from
/// that window is denied if the two drift.
pub fn gateway_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// Where [`launch`] should point the main window instead of the gateway root.
/// Set at most once, by a native notification tap arriving while the client is
/// still starting.
///
/// A tap has to land in the workspace that raised the banner, and during
/// startup there is nothing to land in: [`launch`] owns the first navigation,
/// so pointing the window here ourselves would be clobbered a moment later.
/// Leaving the destination for [`launch`] is what opens the client ON that
/// workspace rather than on the picker plus a second window.
static LAUNCH_TARGET: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Aim [`launch`]'s first navigation at `url`. Last writer wins: two taps before
/// the gateway is up can only produce one landing, and the most recent is the
/// one the user just asked for.
///
/// macOS-gated because its one caller is `crate::app_window::route_native_tap`, and native
/// banners exist only there. Off macOS nothing sets it and
/// [`take_launch_target`] just keeps answering `None`.
#[cfg(target_os = "macos")]
pub fn set_launch_target(url: String) {
    *LAUNCH_TARGET.lock().unwrap() = Some(url);
}

/// Take the pending launch target, if any. Consuming (rather than peeking) is
/// what keeps it to the FIRST navigation: a tap arriving after the window is
/// live is routed by `crate::app_window::route_native_tap` against the real window list
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
/// [`launch`] navigates the window off the Tauri app URL and onto the gateway,
/// which Tauri ACL-checks as `Origin::Remote`. Without this capability EVERY
/// command is rejected. ADR 0028 records the decision and its scoping: a
/// resolved port rather than a wildcard, `webviews` rather than `windows`, and
/// `local(false)`.
///
/// **Read this before widening [`GATEWAY_PERMISSIONS`].** The set includes
/// `updater:default`, so anything answering on that port can drive a signed
/// bundle swap and a full stack restart. The residual is accepted, and ADR 0028
/// weighs both hardenings. The question to ask of a new entry is not whether
/// the frontend needs it, but what it hands to whoever answers on that port.
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
/// gateway. No-op in development. The ensure-and-wait runs on a background
/// thread so the window paints immediately, and the window is navigated once
/// the gateway is healthy.
pub fn launch(app: &AppHandle, nudge_rx: std::sync::mpsc::Receiver<()>) {
    if tauri::is_dev() {
        // No dock-badge thread in dev (unbundled; dev uses the browser) — drop the
        // receiver so the managed sender's `send` is a harmless no-op.
        drop(nudge_rx);
        return;
    }

    // Mirror the gateway's aggregate unread total onto the tray title, and onto
    // the Dock badge while a window is open. Its own thread, independent of the
    // navigate flow below and of whichever page the webview shows. The count
    // therefore reflects the TOTAL rather than the active workspace. The AppKit
    // write is marshalled to the main thread, and the fetch tolerates a gateway
    // that is not up yet.
    //
    // Event-driven AND polled. A nudge recomputes at once, so a notification
    // read anywhere updates the count without waiting. The periodic tick is the
    // safety net for BACKGROUND-workspace changes, whose SSE this webview never
    // sees.
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
                            crate::activation::apply_unread_indicator(&h, total);
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

        // Grant the gateway origin its IPC permissions BEFORE anything can put
        // a window on it. The ACL is consulted per invoke, so registering here
        // covers the navigation below and every later New Window.
        //
        // A permission identifier that does not resolve is caught at TEST time
        // by `acl_tests`, so this arm is a backstop. It logs and still
        // navigates on purpose: a reachable UI with a dead bridge beats
        // stranding the user on the splash with no explanation. The page's own
        // `utils/ipcHealth.ts` then reports the dead bridge to the engine log.
        if let Err(e) = handle.add_capability(gateway_capability(port)) {
            eprintln!(
                "[desktop] FAILED to register the gateway ACL capability for {} — every Tauri IPC \
                 call from the window will be rejected by the ACL: {e}",
                gateway_url(port)
            );
        }

        // Keep the service up and navigate the moment the gateway is healthy,
        // and NEVER permanently give up. A slow start after a forced shutdown,
        // or a transient crash-respawn, can exceed a single wait. Retrying and
        // re-ensuring is what resolves the window whenever the service comes
        // up, rather than stranding the user on the splash.
        //
        // Each cycle re-ensures the LaunchAgent with a bare kickstart. That is
        // a no-op on a still-starting service and a restart on a crashed one,
        // so it never interrupts a slow but progressing start.
        // `wait_for_health` sleeps between attempts, so this cannot busy-loop.
        //
        // Each step also tells `StartupStatus` where it is, the only thing the
        // splash on the other side of the IPC bridge can read.
        //
        // The wait ALSO reads the service's error log back. Its fatal boot
        // checks are correct to be fatal. But launchd's `KeepAlive` turns one
        // into a silent respawn loop. Without this read-back the splash counts
        // up forever at a condition that will never clear.
        let status = handle.state::<StartupStatus>();
        let tail = ServiceLogTail::start(&app_data.join("logs").join(SERVICE_ERR_LOG));
        let watch = ServiceWatch {
            tail: &tail,
            status: &status,
        };
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
            if wait_for_health(port, HEALTH_ENSURE_CYCLE, &watch) {
                break;
            }
            eprintln!(
                "[desktop] gateway not healthy yet on port {port}; re-ensuring service and retrying"
            );
        }

        // Where the windows go. A notification tap outranks the restored
        // session for `main`: the user asked for that workspace just now, and
        // the session is only what they had last time.
        let origin = gateway_url(port);
        let restore = crate::window_persist::resolve_window_session_plan();
        let mut plan = launch_plan(take_launch_target(), restore, &origin);
        navigate_main_window(&handle, &plan.main);
        // A tap that landed between the take above and the navigate would
        // otherwise be stranded on a window already pointed elsewhere. Cheap to
        // re-check, and it closes the only window in which the aim can be lost.
        //
        // The whole plan is recomputed, not just `main`. The tap displaces the
        // workspace `main` was about to take, which has to go back to the
        // extras. And the tapped workspace has to leave them, or it opens twice.
        if let Some(url) = take_launch_target() {
            plan = launch_plan(Some(url), restore, &origin);
            navigate_main_window(&handle, &plan.main);
        }
        // The rest of the session. Building a window is a main-thread call, and
        // this runs on the launch thread.
        if !plan.extra.is_empty() {
            let extra = plan.extra;
            let windows = handle.clone();
            if let Err(e) = handle.run_on_main_thread(move || {
                crate::app_window::restore_extra_windows(&windows, &extra);
            }) {
                eprintln!("[desktop] could not restore the other windows: {e}");
            }
        }
    });
}

/// One additional window this launch opens.
///
/// A distinct type from the `(slug, frame)` pairs `window_session::restore_plan`
/// answers with, so the two cannot be swapped. The slug-to-URL step in
/// [`launch_plan`] is the security boundary (ADR 0028). These types are what
/// keep a slug read off disk from reaching a webview verbatim.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlannedWindow {
    /// The URL to load, composed here from a validated slug.
    pub url: String,
    /// The frame the workspace was last left at, when one is remembered.
    pub frame: Option<crate::window_restore::Rect>,
}

/// Where `main` goes, and what other windows this launch opens.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LaunchPlan {
    /// The URL to navigate `main` to.
    pub main: String,
    pub extra: Vec<PlannedWindow>,
}

/// Decide the launch's windows. Pure, so the precedence is testable.
///
/// Three rules, in order.
///
/// A notification `tap` wins `main`, because the user asked for that workspace
/// a moment ago. Its workspace is then skipped in the restored list, or the tap
/// would open a second window on the one it just landed.
///
/// Otherwise `main` takes the first restored workspace. With none it falls back
/// to the gateway root, the picker, which is the behaviour before any of this.
///
/// Every URL is composed here from a slug `window_session::restore_plan`
/// already validated, so nothing read off disk reaches a webview verbatim.
///
/// A tap also DROPS the first restored workspace's frame. `setup` sized `main`
/// from it before the tap existed, so leaving it would open the extra window
/// exactly on top of `main`.
pub(crate) fn launch_plan(
    tap: Option<String>,
    restore: &[(String, Option<crate::window_restore::Rect>)],
    origin: &str,
) -> LaunchPlan {
    let tapped = tap
        .as_deref()
        .and_then(crate::window_target::window_workspace)
        .map(str::to_string);
    let mut wanted: Vec<(&String, Option<crate::window_restore::Rect>)> = restore
        .iter()
        .filter(|(id, _)| Some(id) != tapped.as_ref())
        .map(|(id, frame)| (id, *frame))
        .collect();

    let main = match tap {
        Some(url) => {
            // `setup` sized `main` from `restore[0]` before this tap existed.
            // When THAT workspace survives as an extra, its window must not
            // reuse the frame `main` now wears, or the two land exactly on top.
            //
            // Keyed on `restore[0]`, not on whatever is first after filtering:
            // a tap ON `restore[0]` displaces nothing, and clearing the next
            // entry's frame would shrink a window for no reason.
            let sized_for = restore.first().map(|(id, _)| id);
            if let Some(entry) = wanted.first_mut() {
                if Some(entry.0) == sized_for {
                    entry.1 = None;
                }
            }
            url
        }
        None if wanted.is_empty() => origin.to_string(),
        None => {
            let (id, _) = wanted.remove(0);
            crate::window_target::workspace_url(origin, id)
        }
    };
    LaunchPlan {
        main,
        extra: wanted
            .into_iter()
            .map(|(id, frame)| PlannedWindow {
                url: crate::window_target::workspace_url(origin, id),
                frame,
            })
            .collect(),
    }
}

/// One live app window, as a reopen sees it.
///
/// `visible` is the field that matters. Close to Menu Bar HIDES every window
/// rather than destroying any, so the hidden set IS the arrangement waiting to
/// come back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveWindow {
    pub label: String,
    pub url: String,
    pub visible: bool,
}

/// What a reopen owes the user.
///
/// Two sources, because a reopen meets the client in two states. A PARKED client
/// still holds every window, hidden, and owes only a show. A client that never
/// restored owes windows it does not have. That second state is the login
/// agent's launch, which comes up menu-bar-only and restores nothing (ADR 0072).
/// So the first reopen after a reboot is where the arrangement is finally owed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ReopenPlan {
    /// Hidden windows to show, `main` first and then by label.
    pub show: Vec<String>,
    /// Where to point `main` first, when it is on no workspace and the record
    /// still names one, and the frame that workspace was left at.
    ///
    /// A `PlannedWindow` rather than a bare URL, because this IS one: the same
    /// workspace, composed the same way, wearing the same remembered frame. It
    /// differs from `build` only in reusing a window instead of making one.
    pub navigate_main: Option<PlannedWindow>,
    /// Recorded workspaces no live window is on.
    pub build: Vec<PlannedWindow>,
    /// The window to front LAST, so it ends on top. `None` only when this
    /// process holds no app window at all, and the caller has to make one.
    pub front: Option<String>,
}

/// Decide what a reopen puts back. Pure, so the precedence is testable.
///
/// A live window always beats the record, in both directions. One that is
/// hidden is shown rather than rebuilt, which is what preserves its page state
/// and makes a park cheap to undo. One that is already visible is left exactly
/// where it is.
///
/// A workspace the record names and no live window is on is BUILT. An adrift
/// `main` takes the first of those as a navigate instead. Otherwise the reopen
/// leaves a picker window sitting behind the restored ones. It carries that
/// workspace's frame like any other owed window: `main` here is the same window
/// `setup` sizes from the same record, and a reopen owes what a launch owes.
///
/// Every URL is composed here from a slug validated by `is_workspace_slug`.
/// Every `window-*` webview holds the full IPC permission set on the gateway
/// origin (ADR 0028), so nothing read off the record may reach one verbatim.
pub(crate) fn reopen_plan(
    live: &[LiveWindow],
    session: &crate::window_session::WindowSession,
    origin: &str,
) -> ReopenPlan {
    let mut ordered: Vec<&LiveWindow> = live.iter().collect();
    ordered.sort_by(|a, b| {
        crate::app_window::window_order_key(&a.label)
            .cmp(&crate::app_window::window_order_key(&b.label))
    });

    let show: Vec<String> = ordered
        .iter()
        .filter(|w| !w.visible)
        .map(|w| w.label.clone())
        .collect();

    let main = ordered
        .iter()
        .find(|w| w.label == crate::app_window::MAIN_WINDOW_LABEL);
    // A `main` still on the bundled splash means `launch` has not reached the
    // gateway yet, and it owns that window's first navigation. It is about to
    // restore these same workspaces, so consulting the record here would
    // navigate twice and build a duplicate. A workspace URL asked for before
    // the gateway is healthy does not even load. A reopen mid-boot therefore
    // shows what is parked and leaves the record alone, the same deference
    // `route_native_tap` shows when it AIMS the boot navigation.
    //
    // This is the likeliest moment of all after a reboot: the login agent sits
    // in the menu bar while the service starts, and the user clicks it.
    let booting = main.is_some_and(|w| !crate::window_target::window_is_navigated(&w.url));

    let mut owed: Vec<(&str, Option<crate::window_restore::Rect>)> = Vec::new();
    if !booting {
        // Every workspace this process is already serving, visible or parked.
        let served: Vec<&str> = live
            .iter()
            .filter_map(|w| crate::window_target::window_workspace(&w.url))
            .collect();
        for id in &session.open {
            let id = id.as_str();
            let known = served.contains(&id) || owed.iter().any(|(seen, _)| *seen == id);
            if crate::window_target::is_workspace_slug(id) && !known {
                owed.push((id, session.geometry.get(id).copied()));
            }
        }
    }

    // Adrift: on the picker or the gateway root, i.e. navigated but on no
    // workspace. Such a window is the one to point somewhere, never to leave.
    let adrift = main.is_some_and(|w| crate::window_target::window_workspace(&w.url).is_none());
    let navigate_main = (adrift && !owed.is_empty()).then(|| {
        let (id, frame) = owed.remove(0);
        PlannedWindow {
            url: crate::window_target::workspace_url(origin, id),
            frame,
        }
    });

    ReopenPlan {
        front: main
            .map(|w| w.label.clone())
            .or_else(|| show.first().cloned()),
        show,
        navigate_main,
        build: owed
            .into_iter()
            .map(|(id, frame)| PlannedWindow {
                url: crate::window_target::workspace_url(origin, id),
                frame,
            })
            .collect(),
    }
}

/// Point the declared main window at `url`. Best-effort: a missing window or
/// an unparseable URL is logged, never fatal. The alternative is stranding the
/// user on the boot splash with no explanation.
pub(crate) fn navigate_main_window(app: &AppHandle, url: &str) {
    // By webview, not webview window, per ADR 0140. A blind lookup leaves an
    // adrift `main` on the picker whenever it has a URL preview open.
    match (
        app.get_webview(crate::app_window::MAIN_WINDOW_LABEL),
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

/// The client's read-back of the service log, and where its verdict lands.
/// Passed as one thing so the wait cannot be given a tail without a status to
/// tell about it.
struct ServiceWatch<'a> {
    tail: &'a ServiceLogTail,
    status: &'a StartupStatus,
}

/// Block until the gateway health endpoint answers 200, or the deadline passes.
/// The gateway serves health behind the sigil namespace (`/~/api/v1/health`,
/// ADR 0014) — a bare `/api/v1/health` would be resolved as a workspace slug.
///
/// It re-reads the service's error log as it waits, on its own slower cadence.
/// Doing that HERE rather than between cycles is what puts the report on the
/// splash seconds after the failure that earned it. A crash loop cycles every
/// `ThrottleInterval` (10s), well inside one [`HEALTH_ENSURE_CYCLE`].
fn wait_for_health(port: u16, timeout: Duration, watch: &ServiceWatch) -> bool {
    let deadline = Instant::now() + timeout;
    let mut next_read = Instant::now();
    while Instant::now() < deadline {
        if http_ok(port, "/~/api/v1/health") {
            return true;
        }
        if Instant::now() >= next_read {
            next_read = Instant::now() + SERVICE_LOG_POLL;
            watch
                .status
                .note_service_boots(&watch.tail.read(), &watch.tail.path);
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
/// answered and only then exited has done its job here: the supervise loop
/// notices the exit and shuts down for a launchd respawn. That is a different
/// and correct outcome from reporting that the boot failed.
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
/// The deadline exists for a gateway that is slow but PROGRESSING, and it still
/// applies to one. What it must not govern is a gateway that has already
/// exited. A bind failure kills the process in under a second. The packaged
/// window would then spend the rest of the deadline on its startup splash for
/// nothing. Watching the child collapses that to the respawn throttle.
fn await_gateway_start(port: u16, timeout: Duration, gateway: &mut Child) -> GatewayStart {
    let deadline = Instant::now() + timeout;
    loop {
        // A failed `try_wait` means we can no longer tell whether the child is
        // alive. Treat that as gone, rather than waiting out the deadline on a
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

/// Record why this service start failed, and hand back the exit code. Every
/// fatal arm of [`run_service`] goes through here, so the line carries
/// [`BOOT_FAILED_MARKER`] and the client can find it. Returning the code keeps
/// the marker and the exit on one statement, which is what stops a new arm
/// exiting silently.
fn service_boot_failed(reason: impl std::fmt::Display) -> i32 {
    eprintln!("[service] {BOOT_FAILED_MARKER} {reason}");
    1
}

/// Headless launchd entry point (`Lucidos --service`). Boots the standalone
/// gateway on the stable port and supervises it, never touching AppKit/Tauri
/// (so no window, no dock icon). Returns the process exit code:
///  * `0` — graceful stop (SIGTERM from `bootout` / `kickstart -k`), or the
///    gateway exited and launchd's `KeepAlive` should respawn us.
///  * non-zero — boot failed; launchd respawns after `ThrottleInterval`.
pub fn run_service() -> i32 {
    // Before the work, and before anything that can fail. The client counts
    // these markers in the error log, so one has to be written per launchd
    // start whatever the start goes on to do.
    eprintln!("{SERVICE_BOOT_MARKER} (pid {})", std::process::id());

    // FIRST, before anything else in the process. Everything below inherits
    // what we set here: the gateway, every workspace engine, every coding
    // agent. See `shell_env` for what launchd leaves out and why.
    //
    // At the top rather than beside `spawn_gateway` because it sets process
    // env, which is only sound while the process is single-threaded. `main`
    // reaches here before any Tauri, AppKit or thread setup.
    #[cfg(target_os = "macos")]
    crate::shell_env::hydrate_login_shell_env();

    install_stop_handlers();

    let app_data = match app_data_dir_from_env() {
        Ok(p) => p,
        Err(e) => return service_boot_failed(format!("cannot resolve app_data_dir: {e}")),
    };
    let resources = match resource_dir_from_exe() {
        Ok(p) => p,
        Err(e) => return service_boot_failed(format!("cannot resolve resource dir: {e}")),
    };
    let port = resolve_engine_port(&app_data);

    let mut svc = match spawn_gateway(&resources, &app_data, port) {
        Ok(svc) => svc,
        Err(e) => {
            return service_boot_failed(format!("could not start the workspace gateway: {e}"))
        }
    };
    match await_gateway_start(port, ENGINE_HEALTH_TIMEOUT, &mut svc.gateway) {
        GatewayStart::Healthy => {}
        GatewayStart::ChildExited => {
            // Say WHICH of the two ways the start failed. The gateway logs its
            // own reason to the same file, immediately above this line, and the
            // client's parser prefers that one. Naming the exit points a human
            // reader at it too, rather than at a timeout that never ran.
            let outcome = service_boot_failed(format!(
                "the gateway exited before answering on port {port}; see its own reason above"
            ));
            svc.shutdown(&resources, &app_data);
            return outcome;
        }
        GatewayStart::TimedOut => {
            // The other half, and the one a genuinely slow machine reaches:
            // still alive, still not answering. The wording is what lets the
            // splash say which of the two it is looking at.
            let outcome = service_boot_failed(format!(
                "the gateway did not become healthy on port {port} within {}s",
                ENGINE_HEALTH_TIMEOUT.as_secs()
            ));
            svc.shutdown(&resources, &app_data);
            return outcome;
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
/// The plist captures `current_exe()`. A signed and notarized app in
/// `/Applications` has a stable path. An unsigned local test build run from
/// Downloads can be Gatekeeper app-translocated to a random read-only mount,
/// so the captured path later vanishes. Move the `.app` into `/Applications`,
/// or sign it, before relying on the service across reboots.
fn ensure_service_installed_and_running(app_data: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;

    // Best-effort, and deliberately first-and-forgotten. The service is what
    // this function must not fail to deliver. A login agent that did not
    // install costs only a client the user opens by hand.
    #[cfg(target_os = "macos")]
    ensure_login_agent_installed(&exe, app_data);

    let changed = install_or_update_service_plist(&exe, app_data)?;

    if changed && is_service_loaded() {
        // A rewritten definition only takes effect after a reload. Remove the
        // old one, then re-bootstrap UNCONDITIONALLY. A fresh
        // `is_service_loaded()` here can still report the just-booted-out job
        // as loaded before launchd settles, and would kickstart the stale
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
    let err = logs.join(SERVICE_ERR_LOG);
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
/// LaunchServices can still be coming up, and this agent exists so the client
/// is there afterwards. One refused `open` must not be the end of it.
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
/// the app exactly as a double-click does, the same reason ADR 0072 gives for
/// [`relaunch_watcher_script`]. And on an ALREADY-RUNNING client it merely
/// activates that instance, so this job can never produce a second client.
/// `-g` keeps that activation out of the foreground: a login start belongs in
/// the menu bar, not in front of whatever the user is doing.
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

/// Pure: what the login agent's plist should contain for this executable. It is
/// `None` when the binary is not inside a `.app`, so LaunchServices would have
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
///    System Settings records a launchd override keyed by the label. The
///    idempotent write does not clear it and nothing here runs `launchctl
///    enable`, so the user's "off" survives every later client launch. A
///    bootstrap is attempted only on a first install or a moved bundle.
///
/// Every failure is logged and swallowed. The client is already running, so the
/// worst case is a missing login agent.
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

/// Restart the gateway service in place. The supervisor catches the SIGTERM,
/// tears the stack down gracefully, and launchd respawns it. Used by the
/// packaged "Restart" control. In development there is no service, so this
/// returns an error the caller can show.
pub fn restart_service() -> Result<(), String> {
    if tauri::is_dev() {
        return Err("Gateway service restart is only available in a packaged build".to_string());
    }
    kickstart_service(true).map_err(|e| e.to_string())
}

/// Stop the always-on service entirely, the explicit "Quit and Stop Background
/// Service" path. Removes the agent so it will not respawn, and the next GUI
/// launch re-installs and re-bootstraps it. No-op in development.
///
/// The ONE teardown that means *stay down*, so it declares that first. The
/// service's own teardown otherwise records its workspaces for the next boot to
/// restore, which is right for a restart and wrong here. Declaring before the
/// `bootout`, rather than clearing the record after it, is what keeps the
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
        // silence the NEXT restart's restore list, so take it back.
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
/// This bounds the WATCHER's life, not the relaunch. It is deliberately far
/// longer than a shutdown takes, because giving up is giving up for good. ADR
/// 0072 records why the bound exists and why reaching it must not launch.
#[cfg(target_os = "macos")]
const RELAUNCH_WAIT_PROBES: u32 = 3000;

/// This client's argv, minus [`LOGIN_FLAG`], for handing to a relaunch of
/// itself.
///
/// The flag is one-shot launch CONTEXT, never a mode the process keeps, and
/// both relaunch paths forward argv verbatim. Without this filter a restarted
/// client comes back hidden and menu-bar-only. ADR 0072 has the rest.
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
/// exited. `Ok` means it is arranged and the caller MUST exit. `Err` means it
/// is not, and the caller must fall back to respawning the executable itself.
///
/// ADR 0072 records why this goes through LaunchServices, why the watcher waits
/// for our exit, and why an unbundled dev binary needs no special case.
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
/// unconditionally, because the loop can also end at its ceiling. ADR 0072
/// records what launching there would cost.
///
/// Errors on a path or argument that is not valid UTF-8: it cannot be quoted
/// into a shell word without corrupting it, and the caller's fallback passes
/// `OsString`s through faithfully.
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

/// Fully remove the bundled Lucidos install from the GUI, so a non-developer
/// never needs a terminal. Stops the service and embedded Postgres, removes the
/// launchd agent, deletes the support data, and trashes the running `.app`.
/// `delete_data` is what also clears the database, the workspaces and the
/// WKWebView web storage, which is what makes a reinstall start clean (see
/// `support_data_paths`).
///
/// Returns `Ok(())` only when the CRITICAL steps succeeded: booting out the
/// service, and every attempted support-data deletion. Stopping engines,
/// deleting the plist and trashing the bundle are best-effort. Their failures
/// are logged and joined into any returned `Err`, but do not on their own fail
/// the uninstall.
///
/// Touches ONLY the bundled install's paths, never the developer dev-setup dirs.
#[cfg(target_os = "macos")]
pub fn uninstall(app_data: &Path, delete_data: bool) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();

    // (a) Stop per-workspace engines. Best-effort, and logged internally.
    stop_workspace_engines(app_data);

    // (b) Stop the embedded Postgres cluster BEFORE deleting its data dir, so
    //     no running postmaster holds the tree.
    match resource_dir_from_exe() {
        Ok(resources) => stop_embedded_postgres(&resources, app_data),
        Err(e) => {
            eprintln!("[service] uninstall: cannot resolve resources to stop Postgres: {e}")
        }
    }

    // (c) Stop and unload the launchd agent (CRITICAL). `bootout` errors when
    //     the job is not loaded, which is the goal already met rather than a
    //     failure, so only bootout what is actually loaded.
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

    // (c2) Boot out the login agent too, best-effort. It is a one-shot job that
    //      has normally run and exited. While it stays loaded, a `kickstart`
    //      could still fire it at the bundle we are about to trash.
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

    // (d) Delete BOTH LaunchAgent plists, so neither can reload at login. The
    //     login one would otherwise spend a boot trying to `open` a bundle
    //     sitting in the Trash.
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

    // (f) Move the running `.app` bundle to the Trash, best-effort, derived
    //     from the current exe so it trashes wherever the app lives. Done LAST,
    //     so `current_exe()` stays valid for the resource resolution above.
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
/// Support data dir AND the WKWebView web-storage trees go only when
/// `delete_data`. The ephemeral caches, preferences and saved window state
/// always go.
///
/// The WebKit trees are load-bearing for a clean reinstall. They hold the
/// embedded WebView's `localStorage`, including the device-global
/// last-workspace key, and the registered SERVICE WORKER with its caches. Left
/// behind, the stale key drives the next cold start at a deleted workspace
/// slug, and the surviving worker serves its cached shell. The app then wedges
/// on the boot splash instead of reaching the picker. The keep-data path leaves
/// them intact, because the workspaces survive and the memory is still valid.
#[cfg(target_os = "macos")]
fn support_data_paths(home: &Path, app_data: &Path, delete_data: bool) -> Vec<PathBuf> {
    let library = home.join("Library");
    let mut paths = Vec::new();
    if delete_data {
        // Passed in, so it matches exactly what the service uses.
        paths.push(app_data.to_path_buf());
        // The WKWebView web storage. Removing it is what makes a reinstall
        // clean, per the doc comment above. Both bundle names, like Caches.
        paths.push(library.join("WebKit").join(BUNDLE_IDENTIFIER));
        paths.push(library.join("WebKit").join("lucidos-app"));
        // Disk HTTP cache and cookies. `delete_path` treats a path that is not
        // on this machine as success.
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
/// parents up. `None` for an unbundled binary, so the caller skips the removal.
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

/// Delete a file or directory tree. A missing path is success, since that is
/// the goal already met. Uses `symlink_metadata`, so a symlink is removed as a
/// link rather than followed.
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

/// Move the bundle to the Trash via Finder, so it is recoverable rather than
/// permanently deleted.
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
/// app-data dir, configured for the EMBEDDED Postgres backend.
///
/// The gateway owns the rest. It creates the workspace registry, provisions one
/// shared Postgres cluster with a database per workspace, spawns each
/// workspace's engine, and reverse-proxies `/<slug>/`. Every engine inherits
/// this environment, so the paths set below reach all of them. First run
/// creates no workspace, and the smart root serves the picker instead.
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
        .env("LUCIDOS_CLI_BIN", &bundle.cli_bin)
        .env("LUCIDOS_STATIC_DIR", &bundle.frontend)
        .env("LUCIDOS_SDK_DIR", &bundle.sdk)
        .env("LUCIDOS_SYSTEM_KNOWHOW_DIR", &bundle.system_knowhow)
        .env("FASTEMBED_CACHE_DIR", &fastembed_cache)
        // Engines may boot before the user has configured a provider, into a
        // clear no-provider onboarding state and NOT mock output.
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
/// HTTP/1.0 request serves both the dock-badge poll and the pairing mint.
///
/// It attaches the machine-local token, which is how this process proves it is
/// local. That is the whole reason the calls come through here rather than
/// through the page: a browser cannot read a mode 0600 file, and a loopback
/// peer address proves nothing, since `tailscale serve` proxies remote requests
/// from that same address.
pub(crate) fn gateway_body(port: u16, method: &str, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let credential = match lucidos_local_token::read() {
        Some(token) => format!("{}: {token}\r\n", lucidos_local_token::HEADER_LOCAL_TOKEN),
        None => String::new(),
    };
    // A bodyless POST still needs a length, or the server waits for one.
    let length = if method == "GET" {
        String::new()
    } else {
        "Content-Length: 0\r\n".to_string()
    };
    let req = format!(
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n{credential}{length}Connection: close\r\n\r\n"
    );
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
    let body = gateway_body(port, "GET", "/~/api/v1/control/unread-total")?;
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

    // ── What a launch reopens ────────────────────────────────────────────────

    fn rect(x: i64, y: i64, width: i64, height: i64) -> crate::window_restore::Rect {
        crate::window_restore::Rect {
            x,
            y,
            width,
            height,
        }
    }

    const ORIGIN: &str = "http://localhost:3210";

    /// A `PlannedWindow`, so each expectation reads as one line.
    fn planned(url: &str, frame: Option<crate::window_restore::Rect>) -> PlannedWindow {
        PlannedWindow {
            url: url.to_string(),
            frame,
        }
    }

    // Before any of this, `main` went to the gateway root and the page
    // redirected to whatever `localStorage` remembered. That is still the
    // answer when there is nothing to restore.
    #[test]
    fn an_empty_session_leaves_main_on_the_picker() {
        let plan = launch_plan(None, &[], ORIGIN);
        assert_eq!(plan.main, ORIGIN);
        assert!(plan.extra.is_empty());
    }

    // The reported defect: two workspaces open, one came back.
    #[test]
    fn every_recorded_workspace_gets_a_window() {
        let plan = launch_plan(
            None,
            &[
                ("myws".to_string(), Some(rect(0, 0, 1200, 800))),
                ("dev".to_string(), Some(rect(10, 20, 900, 700))),
            ],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/myws/");
        assert_eq!(
            plan.extra,
            vec![planned(
                "http://localhost:3210/dev/",
                Some(rect(10, 20, 900, 700))
            )]
        );
    }

    // A workspace with no remembered frame still gets its window, at the
    // default size. Forgetting the size must not cost the window.
    #[test]
    fn a_workspace_with_no_remembered_frame_still_gets_a_window() {
        let plan = launch_plan(
            None,
            &[("myws".to_string(), None), ("dev".to_string(), None)],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/myws/");
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/dev/", None)]
        );
    }

    // The user asked for that workspace a moment ago. The session is only what
    // they had last time, so the tap outranks it.
    #[test]
    fn a_notification_tap_wins_the_main_window() {
        let plan = launch_plan(
            Some("http://localhost:3210/tapped/".to_string()),
            &[("myws".to_string(), None)],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/tapped/");
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/myws/", None)]
        );
    }

    // Otherwise the tap's own workspace would come back twice: once because it
    // was tapped, once because it was open last time.
    #[test]
    fn the_tapped_workspace_is_not_restored_a_second_time() {
        let plan = launch_plan(
            Some("http://localhost:3210/myws/".to_string()),
            &[("myws".to_string(), None), ("dev".to_string(), None)],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/myws/");
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/dev/", None)]
        );
    }

    // A tap can name no workspace at all: the picker, or a URL the gateway
    // would not resolve. It still aims `main`, and nothing is skipped.
    #[test]
    fn a_tap_on_no_workspace_skips_nothing() {
        let plan = launch_plan(
            Some("http://localhost:3210/~/".to_string()),
            &[("myws".to_string(), None)],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/~/");
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/myws/", None)]
        );
    }

    // A tap can land between the first `take_launch_target` and the navigate,
    // and `launch` recomputes the WHOLE plan for it rather than only `main`.
    // Recomputing is what these two assert: the displaced workspace comes back
    // as an extra, and the tapped one leaves the extras.
    #[test]
    fn a_late_tap_returns_the_workspace_it_displaced_to_the_extras() {
        let restore = [
            ("myws".to_string(), Some(rect(0, 0, 1200, 800))),
            ("dev".to_string(), None),
        ];
        let first = launch_plan(None, &restore, ORIGIN);
        assert_eq!(first.main, "http://localhost:3210/myws/");

        let after = launch_plan(
            Some("http://localhost:3210/tapped/".to_string()),
            &restore,
            ORIGIN,
        );
        assert_eq!(after.main, "http://localhost:3210/tapped/");
        assert_eq!(
            after.extra,
            vec![
                // Frameless on purpose: `setup` already sized `main` from this
                // workspace's rect, so reusing it would stack the two windows
                // exactly on top of each other.
                planned("http://localhost:3210/myws/", None),
                planned("http://localhost:3210/dev/", None),
            ],
            "the workspace the tap displaced must still get a window",
        );
    }

    #[test]
    fn a_late_tap_on_a_restored_workspace_does_not_open_it_twice() {
        let restore = [("myws".to_string(), None), ("dev".to_string(), None)];
        let after = launch_plan(
            Some("http://localhost:3210/dev/".to_string()),
            &restore,
            ORIGIN,
        );
        assert_eq!(after.main, "http://localhost:3210/dev/");
        assert_eq!(
            after.extra,
            vec![planned("http://localhost:3210/myws/", None)]
        );
    }

    // A tap on `restore[0]` displaces nobody: `main` was sized for that same
    // workspace and now shows it. Clearing the NEXT entry's frame, which is
    // what keying the drop on the filtered list did, shrinks a window for
    // nothing.
    #[test]
    fn a_tap_on_the_first_restored_workspace_costs_no_other_window_its_size() {
        let restore = [
            ("myws".to_string(), Some(rect(0, 0, 1200, 800))),
            ("dev".to_string(), Some(rect(50, 60, 900, 700))),
        ];
        let plan = launch_plan(
            Some("http://localhost:3210/myws/".to_string()),
            &restore,
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/myws/");
        assert_eq!(
            plan.extra,
            vec![planned(
                "http://localhost:3210/dev/",
                Some(rect(50, 60, 900, 700))
            )]
        );
    }

    // A client reached over a tailnet address opens its restored windows there
    // too, the same rule `workspace_window_url` follows.
    #[test]
    fn the_restored_windows_follow_the_origin_they_are_given() {
        let plan = launch_plan(
            None,
            &[("myws".to_string(), None), ("dev".to_string(), None)],
            "https://box.tail1234.ts.net",
        );
        assert_eq!(plan.main, "https://box.tail1234.ts.net/myws/");
        assert_eq!(
            plan.extra,
            vec![planned("https://box.tail1234.ts.net/dev/", None)]
        );
    }

    // ── What a reopen puts back ──────────────────────────────────────────────

    fn live(label: &str, url: &str, visible: bool) -> LiveWindow {
        LiveWindow {
            label: label.to_string(),
            url: url.to_string(),
            visible,
        }
    }

    /// A record naming `open`, with a frame for each entry `geometry` lists.
    fn session(
        open: &[&str],
        geometry: &[(&str, crate::window_restore::Rect)],
    ) -> crate::window_session::WindowSession {
        crate::window_session::WindowSession {
            open: open.iter().map(|id| id.to_string()).collect(),
            geometry: geometry
                .iter()
                .map(|(id, frame)| (id.to_string(), *frame))
                .collect(),
        }
    }

    // The reported defect. Cmd-Q parks three windows and the reopen brought
    // back `main` alone, because the park had destroyed the other two.
    #[test]
    fn a_reopen_shows_every_parked_window() {
        let plan = reopen_plan(
            &[
                live("main", "http://localhost:3210/myws/", false),
                live("window-1", "http://localhost:3210/dev/", false),
                live("window-2", "http://localhost:3210/other/", false),
            ],
            &session(&["myws", "dev", "other"], &[]),
            ORIGIN,
        );
        assert_eq!(plan.show, vec!["main", "window-1", "window-2"]);
        assert_eq!(plan.front.as_deref(), Some("main"));
        // Every recorded workspace already has a window, so nothing is built.
        assert!(plan.build.is_empty());
        assert_eq!(plan.navigate_main, None);
    }

    // `main` first, then by label. The same order `window_session::capture`
    // writes, through the one `window_order_key`.
    #[test]
    fn the_parked_windows_come_back_in_a_stable_order() {
        let plan = reopen_plan(
            &[
                live("window-2", "http://localhost:3210/c/", false),
                live("main", "http://localhost:3210/a/", false),
                live("window-1", "http://localhost:3210/b/", false),
            ],
            &crate::window_session::WindowSession::default(),
            ORIGIN,
        );
        assert_eq!(plan.show, vec!["main", "window-1", "window-2"]);
    }

    // A window already on screen is where the user put it. The tray item still
    // fronts `main`, which is what it did before any of this.
    #[test]
    fn a_visible_window_is_left_alone() {
        let plan = reopen_plan(
            &[
                live("main", "http://localhost:3210/myws/", true),
                live("window-1", "http://localhost:3210/dev/", false),
            ],
            &session(&["myws", "dev"], &[]),
            ORIGIN,
        );
        assert_eq!(plan.show, vec!["window-1"]);
        assert_eq!(plan.front.as_deref(), Some("main"));
    }

    // The reboot case. The login agent comes up menu-bar-only and restores
    // nothing (ADR 0072), so the first reopen is where the record is owed.
    #[test]
    fn a_reopen_with_nothing_restored_rebuilds_from_the_record() {
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/", false)],
            &session(
                &["myws", "dev"],
                &[
                    ("myws", rect(0, 0, 1200, 800)),
                    ("dev", rect(9, 9, 900, 700)),
                ],
            ),
            ORIGIN,
        );
        // `main` is adrift on the gateway root, so it TAKES the first workspace
        // rather than leaving a picker window behind the restored ones. With
        // that workspace's frame: it is owed exactly what `setup` would have
        // given it, had this launch restored anything.
        assert_eq!(
            plan.navigate_main,
            Some(planned(
                "http://localhost:3210/myws/",
                Some(rect(0, 0, 1200, 800))
            ))
        );
        assert_eq!(
            plan.build,
            vec![planned(
                "http://localhost:3210/dev/",
                Some(rect(9, 9, 900, 700))
            )]
        );
        assert_eq!(plan.show, vec!["main"]);
        assert_eq!(plan.front.as_deref(), Some("main"));
    }

    // A live window always beats the record. Rebuilding a workspace one is
    // already on would double it on every reopen.
    #[test]
    fn a_workspace_a_live_window_is_on_is_never_rebuilt() {
        let plan = reopen_plan(
            &[
                live("main", "http://localhost:3210/myws/", false),
                live("window-1", "http://localhost:3210/dev/", true),
            ],
            &session(&["myws", "dev"], &[]),
            ORIGIN,
        );
        assert!(plan.build.is_empty());
        assert_eq!(plan.navigate_main, None);
    }

    // A `main` already on a workspace is not adrift. It keeps the page it is
    // on, and the record's entry becomes a window of its own.
    #[test]
    fn a_main_on_a_workspace_is_never_navigated_away() {
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/myws/", false)],
            &session(&["dev"], &[]),
            ORIGIN,
        );
        assert_eq!(plan.navigate_main, None);
        assert_eq!(
            plan.build,
            vec![planned("http://localhost:3210/dev/", None)]
        );
    }

    // The record is a file on disk, and every `window-*` webview holds the full
    // IPC permission set on the gateway origin (ADR 0028). A slug that is not
    // one is dropped rather than composed into a URL.
    #[test]
    fn a_reopen_builds_nothing_from_a_slug_the_record_cannot_justify() {
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/myws/", false)],
            &session(
                &["..", "a/b", "MyWs", "~", "", "http://evil.example", "ok"],
                &[],
            ),
            ORIGIN,
        );
        assert_eq!(plan.build, vec![planned("http://localhost:3210/ok/", None)]);
    }

    // Two entries naming one workspace would land the second window exactly on
    // the first, which is the rule `capture` already applies on the way in.
    #[test]
    fn a_repeated_workspace_is_built_once() {
        let plan = reopen_plan(&[], &session(&["dev", "dev"], &[]), ORIGIN);
        assert_eq!(
            plan.build,
            vec![planned("http://localhost:3210/dev/", None)]
        );
    }

    // Nothing at all: no window in the process and nothing recorded. The caller
    // reads the empty `front` as "make one".
    #[test]
    fn a_reopen_with_no_window_and_no_record_leaves_the_caller_to_make_one() {
        let plan = reopen_plan(
            &[],
            &crate::window_session::WindowSession::default(),
            ORIGIN,
        );
        assert_eq!(plan, ReopenPlan::default());
        assert_eq!(plan.front, None);
    }

    // No `main` in the process, so the first parked window is what comes
    // forward. Fronting nothing leaves the client `Regular` with no window.
    #[test]
    fn without_main_the_first_parked_window_is_fronted() {
        let plan = reopen_plan(
            &[live("window-1", "http://localhost:3210/dev/", false)],
            &session(&["dev"], &[]),
            ORIGIN,
        );
        assert_eq!(plan.front.as_deref(), Some("window-1"));
        assert_eq!(plan.navigate_main, None);
        assert!(plan.build.is_empty());
    }

    // `launch` owns `main`'s first navigation and is about to restore these
    // same workspaces. Acting on the record here navigates twice, builds a
    // duplicate, and asks for a workspace URL the gateway cannot serve yet.
    // The likeliest reopen of all lands here: the menu-bar item, clicked while
    // the service is still starting after a reboot.
    #[test]
    fn a_reopen_mid_boot_leaves_the_record_to_the_launch() {
        let plan = reopen_plan(
            &[live("main", "tauri://localhost", false)],
            &session(&["myws", "dev"], &[]),
            ORIGIN,
        );
        assert_eq!(plan.navigate_main, None);
        assert!(plan.build.is_empty());
        // The parked window still comes back, which is all a reopen owes here.
        assert_eq!(plan.show, vec!["main"]);
        assert_eq!(plan.front.as_deref(), Some("main"));
    }

    // Boot is over the moment `main` reaches the gateway, even on the root.
    // That is the login start, and its first reopen is owed the arrangement.
    #[test]
    fn a_main_on_the_gateway_root_is_past_boot() {
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/~/", false)],
            &session(&["myws"], &[]),
            ORIGIN,
        );
        // Nothing recorded about its size, so it keeps the one it has.
        assert_eq!(
            plan.navigate_main,
            Some(planned("http://localhost:3210/myws/", None))
        );
    }

    // A client reached over a tailnet address rebuilds its windows there too,
    // the same rule `launch_plan` follows.
    #[test]
    fn a_rebuilt_window_follows_the_origin_it_is_given() {
        let plan = reopen_plan(&[], &session(&["dev"], &[]), "https://box.tail1234.ts.net");
        assert_eq!(
            plan.build,
            vec![planned("https://box.tail1234.ts.net/dev/", None)]
        );
    }

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
        // the supervise loop's business, which is a different outcome from
        // "this boot never came up".
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
                assert_eq!(startup_label(phase, elapsed, None, None), STARTING_LABEL);
            }
        }
        // Even a failure stays quiet while the start is still young: the loop
        // re-ensures the service immediately, so a cycle that fails and recovers
        // inside the quiet period never troubles the user with it.
        assert_eq!(
            startup_label(
                StartupPhase::WaitingForGateway,
                Duration::from_secs(1),
                Some("Could not start the background service: nope."),
                None
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
            None,
        );
        let at_13s = startup_label(
            StartupPhase::WaitingForGateway,
            Duration::from_secs(13),
            None,
            None,
        );
        assert_eq!(at_12s, "Waiting for the background service… (12s)");
        assert_ne!(at_12s, at_13s, "the line must change as the wait runs");
        assert_eq!(
            startup_label(
                StartupPhase::EnsuringService,
                Duration::from_secs(12),
                None,
                None
            ),
            "Starting the background service…"
        );
    }

    #[test]
    fn a_long_wait_says_a_restart_explains_it() {
        let label = startup_label(
            StartupPhase::WaitingForGateway,
            Duration::from_secs(95),
            None,
            None,
        );
        assert_eq!(
            label,
            "Waiting for the background service… (1m 35s). It may still be starting up after a \
             restart."
        );
        // The boundary belongs to the long form, not the short one.
        assert!(startup_label(
            StartupPhase::WaitingForGateway,
            STARTUP_LONG_WAIT,
            None,
            None
        )
        .contains("after a restart"));
        assert!(!startup_label(
            StartupPhase::WaitingForGateway,
            STARTUP_LONG_WAIT - Duration::from_secs(1),
            None,
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
                Some("Could not start the background service: launchctl bootstrap failed."),
                None
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
        // What this pins: the loop records an ensure failure and moves to
        // WaitingForGateway on the very next line. A phase change that cleared
        // the detail would make the failure text unreachable. That wait is the
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
        // progress through the loop. A service that started on the retry leaves
        // the splash reading as an ordinary wait.
        let status = StartupStatus::default();
        status.note_failure("Could not start the background service: nope.");
        status.enter(StartupPhase::EnsuringService);
        status.clear_failure();
        status.enter(StartupPhase::WaitingForGateway);
        aged(&status, Duration::from_secs(20));

        assert_eq!(status.label(), "Waiting for the background service… (20s)");
    }

    // ── Telling a crash loop from a slow boot ────────────────────────────────

    /// One launchd start of a service whose gateway fails a staging check. The
    /// gateway's own line lands first, because it dies before the service can
    /// notice, and both go to this one file.
    fn crashing_boot(pid: u32) -> String {
        format!(
            "{SERVICE_BOOT_MARKER} (pid {pid})\n\
             [gateway] boot failed: LUCIDOS_STATIC_DIR is set to /R/frontend but its index.html \
             is missing\n\
             [service] {BOOT_FAILED_MARKER} the gateway exited before answering on port 5252; \
             see its own reason above\n"
        )
    }

    #[test]
    fn the_gateways_own_reason_wins_over_the_services_summary() {
        // Both lines describe one event, and only the first names the path. A
        // parser that took the LAST match would show the useless half.
        let report = parse_service_boots(&crashing_boot(101));
        assert_eq!(report.boots, 1);
        assert_eq!(
            report.reason.as_deref(),
            Some("LUCIDOS_STATIC_DIR is set to /R/frontend but its index.html is missing")
        );
    }

    #[test]
    fn a_service_that_fails_before_the_gateway_still_reports() {
        let window = format!(
            "{SERVICE_BOOT_MARKER} (pid 7)\n\
             [service] {BOOT_FAILED_MARKER} cannot resolve resource dir: not a bundle\n"
        );
        let report = parse_service_boots(&window);
        assert_eq!(report.boots, 1);
        assert_eq!(
            report.reason.as_deref(),
            Some("cannot resolve resource dir: not a bundle")
        );
    }

    #[test]
    fn starts_are_counted_and_the_newest_reason_kept() {
        let window = format!(
            "{}{}{}",
            crashing_boot(1),
            crashing_boot(2),
            crashing_boot(3).replace("/R/frontend", "/R2/frontend")
        );
        let report = parse_service_boots(&window);
        assert_eq!(report.boots, 3);
        assert!(report.reason.unwrap().contains("/R2/frontend"));
    }

    #[test]
    fn a_log_that_starts_mid_failure_reports_only_what_it_watched() {
        // The client records the log's length when it starts waiting, so the
        // window can open inside a previous start. That start is not ours to
        // count, and its reason is not ours to show.
        let window = format!(
            "[gateway] boot failed: a reason from before the client opened\n\
             [service] {BOOT_FAILED_MARKER} an old summary\n\
             {SERVICE_BOOT_MARKER} (pid 9)\n"
        );
        let report = parse_service_boots(&window);
        assert_eq!(report.boots, 1);
        assert_eq!(report.reason, None, "the previous start is not ours");
    }

    #[test]
    fn a_start_that_recovered_leaves_no_reason_behind() {
        let window = format!("{}{SERVICE_BOOT_MARKER} (pid 2)\n", crashing_boot(1));
        let report = parse_service_boots(&window);
        assert_eq!(report.boots, 2);
        // The FIRST start failed and the second is still running, so there IS a
        // reason to show. What must not happen is the count stalling at one.
        assert!(report.reason.is_some());
    }

    #[test]
    fn a_healthy_log_says_nothing() {
        let window = format!(
            "{SERVICE_BOOT_MARKER} (pid 4)\n[service] gateway healthy on port 5252; supervising\n"
        );
        assert_eq!(
            parse_service_boots(&window),
            ServiceBootReport {
                boots: 1,
                reason: None
            }
        );
        assert_eq!(parse_service_boots(""), ServiceBootReport::default());
    }

    #[test]
    fn a_log_with_a_bad_byte_in_it_is_still_read() {
        // The service's children share this stderr, and the read is byte-capped,
        // so the window can hold something that is not valid UTF-8. A strict
        // decode would refuse the whole read and turn the report off silently.
        let dir = std::env::temp_dir().join(format!("lucidos-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SERVICE_ERR_LOG);
        let mut bytes = format!("{SERVICE_BOOT_MARKER} (pid 1)\n").into_bytes();
        bytes.extend_from_slice(&[0xff, 0xfe]); // a child's non-UTF-8 output
        bytes.extend_from_slice(b"\n[service] boot failed: no engine binary\n");
        std::fs::write(&path, &bytes).unwrap();

        // Watching starts at zero here, because the file did not exist yet.
        let tail = ServiceLogTail {
            path: path.clone(),
            from: 0,
        };
        assert_eq!(
            tail.read(),
            ServiceBootReport {
                boots: 1,
                reason: Some("no engine binary".to_string())
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_absent_log_reads_as_no_evidence() {
        let tail = ServiceLogTail::start(Path::new("/no/such/engine-service.err.log"));
        assert_eq!(tail.from, 0);
        assert_eq!(tail.read(), ServiceBootReport::default());
    }

    /// The report the splash gets after `boots` starts, with the log where a
    /// packaged install keeps it.
    fn crash(boots: usize, reason: Option<&str>) -> CrashLoop {
        CrashLoop {
            boots,
            reason: reason.map(str::to_string),
            log: PathBuf::from("/Users/me/Library/Application Support/com.lucidos.app/logs")
                .join(SERVICE_ERR_LOG),
        }
    }

    #[test]
    fn a_crash_loop_stops_the_clock_and_says_why() {
        let label = startup_label(
            StartupPhase::WaitingForGateway,
            Duration::from_secs(300),
            None,
            Some(&crash(
                4,
                Some("LUCIDOS_ENGINE_BIN does not exist: /R/lucidos-engine"),
            )),
        );
        assert_eq!(
            label,
            "The background service is not starting. It has started 4 times without coming up.\n\
             LUCIDOS_ENGINE_BIN does not exist: /R/lucidos-engine\n\
             Lucidos keeps trying. Log: /Users/me/Library/Application Support/com.lucidos.app/\
             logs/engine-service.err.log"
        );
        // The counter is gone. That is the whole point: it was counting toward
        // a condition that will never clear.
        assert!(!label.contains("Waiting for the background service"));
        assert!(!label.contains("5m 00s"));
    }

    #[test]
    fn a_crash_loop_with_no_reason_still_names_the_log() {
        let label = startup_label(
            StartupPhase::WaitingForGateway,
            Duration::from_secs(300),
            None,
            Some(&crash(3, None)),
        );
        assert_eq!(label.lines().count(), 2);
        assert!(label.ends_with("logs/engine-service.err.log"));
    }

    #[test]
    fn a_crash_loop_outranks_an_ensure_failure_but_not_the_quiet_period() {
        let detail = Some("Could not start the background service: nope.");
        assert!(startup_label(
            StartupPhase::WaitingForGateway,
            Duration::from_secs(300),
            detail,
            Some(&crash(3, Some("boom"))),
        )
        .starts_with("The background service is not starting."));
        // Three starts cannot happen inside the quiet period, but the ordering
        // is stated here rather than left to arithmetic.
        assert_eq!(
            startup_label(
                StartupPhase::WaitingForGateway,
                Duration::from_secs(1),
                detail,
                Some(&crash(3, Some("boom"))),
            ),
            STARTING_LABEL
        );
    }

    #[test]
    fn a_slow_boot_is_left_alone() {
        // The invariant that matters most here. A cold-machine Postgres initdb
        // writes ONE boot marker and keeps working. The splash must go on
        // counting rather than accusing it of failing.
        let status = StartupStatus::default();
        let log = Path::new("/tmp/does-not-matter.log");
        for boots in [0usize, 1, SERVICE_CRASH_LOOP_BOOTS - 1] {
            status.note_service_boots(
                &ServiceBootReport {
                    boots,
                    reason: Some("the gateway did not become healthy within 120s".to_string()),
                },
                log,
            );
            status.enter(StartupPhase::WaitingForGateway);
            aged(&status, Duration::from_secs(95));
            assert!(
                status
                    .label()
                    .starts_with("Waiting for the background service…"),
                "{boots} starts must still read as a wait"
            );
        }
    }

    #[test]
    fn a_service_that_comes_up_late_drops_the_crash_report() {
        // note_service_boots CLEARS below the threshold. Without that, a service
        // that finally started would leave the accusation on screen.
        let status = StartupStatus::default();
        let log = Path::new("/tmp/does-not-matter.log");
        let failing = ServiceBootReport {
            boots: SERVICE_CRASH_LOOP_BOOTS,
            reason: Some("boom".to_string()),
        };
        status.enter(StartupPhase::WaitingForGateway);
        status.note_service_boots(&failing, log);
        aged(&status, Duration::from_secs(95));
        assert!(status
            .label()
            .starts_with("The background service is not starting."));

        status.note_service_boots(&ServiceBootReport::default(), log);
        assert!(status
            .label()
            .starts_with("Waiting for the background service…"));
    }

    #[test]
    fn a_long_reason_is_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_REASON_CHARS + 40);
        let cut = truncate_reason(&long);
        assert_eq!(cut.chars().count(), MAX_REASON_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(truncate_reason("short"), "short");
    }

    #[test]
    fn the_client_greps_for_the_marker_the_gateway_actually_writes() {
        // `lucidos-app` cannot link `lucidos-gateway` (ADR 0014 §1), so the
        // contract between the two is this file's text. Read it rather than
        // trusting a comment: a reworded gateway line would otherwise take the
        // splash's reason away with nothing going red.
        let gateway_main = include_str!("../../lucidos-gateway/src/main.rs");
        assert!(
            gateway_main.contains(&format!("[gateway] {BOOT_FAILED_MARKER}")),
            "lucidos-gateway must still stamp `[gateway] {BOOT_FAILED_MARKER}` on a fatal boot"
        );
    }

    #[test]
    fn the_reader_and_the_plist_name_one_file() {
        let app_data = Path::new("/Users/me/Library/Application Support/com.lucidos.app");
        let plist = desired_service_plist(Path::new("/Applications/Lucidos.app"), app_data);
        let read_back = app_data.join("logs").join(SERVICE_ERR_LOG);
        assert!(plist.contains(&xml_escape(&read_back)));
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
        // bar" is the symptom this feature exists to fix. The one place that
        // can explain it must not exit silently.
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

        // What this prevents: a client the login agent started keeps `--login`
        // in its argv forever, and both relaunch paths forward argv verbatim.
        // So an update or a Restart App would bring the client back HIDDEN,
        // even with a window open, which reads as the app vanishing.
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
    // that is still alive would only activate it, and the watcher would be gone
    // by the time that client exited. The relaunch would be spent on nothing.
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
    // down, and it says so before it signals. The teardown that follows must
    // leave its marker alone rather than record a restore over it.
    #[test]
    fn a_declared_quit_survives_the_teardown_that_follows_it() {
        let tmp = TempAppData::new("quit");
        declare_quit_intent(tmp.path());
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(NEXT_BOOT_QUIT));
    }

    // A quit intent whose `bootout` failed describes a teardown that never
    // happened, and nothing else clears it: only the gateway's boot consumes
    // the record, and the service is still up. Left behind it would silence the
    // next real restart's restore list.
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
        // A pid that cannot exist, since macOS caps pids well below this. It
        // stands in for a pidfile an engine left behind when it died.
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
