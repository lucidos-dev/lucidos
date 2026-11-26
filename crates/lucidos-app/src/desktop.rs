use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

use lucidos_installs::BUNDLE_IDENTIFIER;

#[cfg(target_os = "macos")]
pub use lucidos_installs::LOGIN_AGENT_LABEL;
pub use lucidos_installs::SERVICE_AGENT_LABEL;

pub const LOGIN_FLAG: &str = "--login";

pub const DEFAULT_ENGINE_PORT: u16 = lucidos_installs::DEFAULT_GATEWAY_PORT;

pub(crate) const GATEWAY_HEALTH_PATH: &str = "/~/api/v1/health";

const ENGINE_HEALTH_TIMEOUT: Duration = Duration::from_secs(120);

const HEALTH_ENSURE_CYCLE: Duration = Duration::from_secs(30);

#[cfg(target_os = "macos")]
const DOCK_BADGE_POLL_INTERVAL: Duration = Duration::from_secs(5);

const STARTUP_QUIET_PERIOD: Duration = Duration::from_secs(8);

const STARTUP_LONG_WAIT: Duration = Duration::from_secs(60);

const STARTING_LABEL: &str = "Starting Lucidos…";

const SERVICE_ERR_LOG: &str = "engine-service.err.log";

const SERVICE_BOOT_MARKER: &str = "[service] boot starting";

const BOOT_FAILED_MARKER: &str = "boot failed:";

const SERVICE_CRASH_LOOP_BOOTS: usize = 3;

const SERVICE_LOG_POLL: Duration = Duration::from_secs(2);

const SERVICE_LOG_MAX_BYTES: u64 = 512 * 1024;

const MAX_REASON_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPhase {
    EnsuringService,
    WaitingForGateway,
}

pub struct StartupStatus {
    inner: std::sync::Mutex<StartupProgress>,
}

struct StartupProgress {
    phase: StartupPhase,
    began: Instant,
    detail: Option<String>,
    crash: Option<CrashLoop>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CrashLoop {
    boots: usize,
    reason: Option<String>,
    log: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupReport {
    pub label: String,
    pub trust_button: Option<&'static str>,
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
    fn enter(&self, phase: StartupPhase) {
        if let Ok(mut p) = self.inner.lock() {
            p.phase = phase;
        }
    }

    fn note_failure(&self, detail: impl Into<String>) {
        if let Ok(mut p) = self.inner.lock() {
            p.detail = Some(detail.into());
        }
    }

    fn clear_failure(&self) {
        if let Ok(mut p) = self.inner.lock() {
            p.detail = None;
        }
    }

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

    pub fn report(&self, trust: &crate::loopback_trust::LoopbackTrust) -> StartupReport {
        trust.prompt().unwrap_or_else(|| StartupReport {
            label: self.label(),
            trust_button: None,
        })
    }

    fn label(&self) -> String {
        match self.inner.lock() {
            Ok(p) => startup_label(
                p.phase,
                p.began.elapsed(),
                p.detail.as_deref(),
                p.crash.as_ref(),
            ),
            Err(_) => STARTING_LABEL.to_string(),
        }
    }
}

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

fn crash_loop_label(crash: &CrashLoop) -> String {
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

fn truncate_reason(reason: &str) -> String {
    match reason.char_indices().nth(MAX_REASON_CHARS) {
        Some((cut, _)) => format!("{}…", &reason[..cut]),
        None => reason.to_string(),
    }
}

fn humanize_wait(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ServiceBootReport {
    boots: usize,
    reason: Option<String>,
}

struct ServiceLogTail {
    path: PathBuf,
    from: u64,
}

impl ServiceLogTail {
    fn start(path: &Path) -> Self {
        let from = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Self {
            path: path.to_path_buf(),
            from,
        }
    }

    fn read(&self) -> ServiceBootReport {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return ServiceBootReport::default();
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let from = if len < self.from { 0 } else { self.from };
        if std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(from)).is_err() {
            return ServiceBootReport::default();
        }
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

fn parse_service_boots(window: &str) -> ServiceBootReport {
    let mut boots = 0usize;
    let mut current: Option<String> = None;
    let mut last_failed: Option<String> = None;

    for line in window.lines() {
        if line.contains(SERVICE_BOOT_MARKER) {
            boots += 1;
            current = None;
            continue;
        }
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
const SYSTEM_KNOWHOW_RESOURCE_NAME: &str = "system-knowhow";
const SYSTEM_WIDGETS_RESOURCE_NAME: &str = "system-widgets";
const CLI_RESOURCE_NAME: &str = "lucidos";

static SERVICE_STOP: AtomicBool = AtomicBool::new(false);

struct GatewayService {
    gateway: Child,
}

impl GatewayService {
    fn shutdown(&mut self, resources: &Path, app_data: &Path) {
        #[cfg(unix)]
        let started = Instant::now();
        #[cfg(unix)]
        {
            // SAFETY: signalling our own still-running child; a pid that has
            // already exited returns ESRCH, which is nothing to act on here.
            unsafe {
                libc::kill(self.gateway.id() as libc::pid_t, libc::SIGUSR1);
            }
            let gateway = &mut self.gateway;
            wait_until_gone(GATEWAY_STOP_TIMEOUT, || {
                matches!(gateway.try_wait(), Ok(Some(_)))
            });
        }
        let _ = self.gateway.kill();
        let _ = self.gateway.wait();
        let stopped = stop_workspace_engines(app_data);
        let mut ids: Vec<String> = stopped.iter().map(|e| e.id.clone()).collect();
        ids.dedup();
        record_workspaces_to_restore(app_data, &ids);
        #[cfg(unix)]
        finish_stopping_engines(&stopped, engine_exit_wait(started.elapsed()));
        stop_embedded_postgres(resources, app_data);
    }
}

#[cfg(unix)]
const GATEWAY_STOP_TIMEOUT: Duration = Duration::from_secs(3);

const LAUNCHD_EXIT_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(unix)]
const SERVICE_TEARDOWN_ALLOWANCE: Duration = Duration::from_secs(3);

#[cfg(unix)]
const ENGINE_KILL_GRACE: Duration = Duration::from_secs(1);

#[cfg(unix)]
fn engine_exit_wait(spent: Duration) -> Duration {
    (LAUNCHD_EXIT_TIMEOUT - SERVICE_TEARDOWN_ALLOWANCE).saturating_sub(spent)
}

#[cfg(unix)]
fn finish_stopping_engines(engines: &[SignalledEngine], timeout: Duration) {
    let mut killed = Vec::new();
    for engine in wait_for_engines_to_exit(engines, timeout) {
        if !pid_runs_engine(engine.pid) {
            eprintln!(
                "[service] pid {} for '{}' no longer runs the engine; not killing it",
                engine.pid, engine.id
            );
            continue;
        }
        eprintln!(
            "[service] engine for '{}' (pid {}) did not exit within {}s; force-killing it",
            engine.id,
            engine.pid,
            timeout.as_secs()
        );
        // SAFETY: a positive pid we signalled, that is alive and runs the engine.
        unsafe {
            libc::kill(engine.pid, libc::SIGKILL);
        }
        killed.push(engine);
    }
    for engine in wait_for_engines_to_exit(&killed, ENGINE_KILL_GRACE) {
        eprintln!(
            "[service] engine for '{}' (pid {}) is still running after SIGKILL",
            engine.id, engine.pid
        );
    }
}

#[cfg(unix)]
fn pid_runs_engine(pid: i32) -> bool {
    pid_executable(pid).is_some_and(|exe| executable_is_engine(&exe))
}

#[cfg(unix)]
fn pid_runs_another_program(pid: i32) -> bool {
    pid_executable(pid).is_some_and(|exe| !executable_is_engine(&exe))
}

#[cfg(target_os = "macos")]
fn pid_executable(pid: i32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable and its length is passed alongside it.
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (len > 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len as usize])))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pid_executable(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(unix)]
fn executable_is_engine(exe: &Path) -> bool {
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.strip_suffix(" (deleted)").unwrap_or(name) == ENGINE_RESOURCE_NAME
}

fn stop_embedded_postgres(resources: &Path, app_data: &Path) {
    let data = app_data.join("pgdata");
    if !data.join("PG_VERSION").exists() {
        return;
    }
    let bundle = bundled_resources(resources);
    match embedded_pg_stop_command(&bundle.pg_bin, &bundle.pg_lib, &data).status() {
        Ok(s) if s.success() => eprintln!("[service] embedded Postgres stopped"),
        Ok(s) => eprintln!("[service] pg_ctl stop exited with {s}"),
        Err(e) => eprintln!("[service] failed to run pg_ctl stop: {e}"),
    }
}

fn embedded_pg_stop_command(pg_bin: &Path, pg_lib: &Path, data: &Path) -> Command {
    let mut cmd = Command::new(pg_bin.join("pg_ctl"));
    cmd.env("DYLD_LIBRARY_PATH", pg_lib);
    cmd.env("LD_LIBRARY_PATH", pg_lib);
    cmd.arg("-D").arg(data);
    cmd.args(["-m", "fast", "-w", "stop"]);
    cmd
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SignalledEngine {
    id: String,
    pid: i32,
}

fn stop_workspace_engines(app_data: &Path) -> Vec<SignalledEngine> {
    let workspaces = app_data.join("workspaces");
    let Ok(entries) = std::fs::read_dir(&workspaces) else {
        return Vec::new();
    };
    let mut stopped = Vec::new();
    for entry in entries.flatten() {
        let waiting = lucidos_owner_lock::engine::waiting_pid(&entry.path())
            .and_then(|pid| i32::try_from(pid).ok());
        for pid in read_engine_pid(&entry.path()).into_iter().chain(waiting) {
            // SAFETY: signal 0 checks for the process's existence without
            // delivering anything. Both reads have already refused the pids
            // that would broadcast.
            #[cfg(unix)]
            if unsafe { libc::kill(pid, 0) } == 0 && !pid_runs_another_program(pid) {
                // SAFETY: a positive, live pid not known to run anything else.
                unsafe {
                    libc::kill(pid, libc::SIGUSR1);
                }
                if let Some(id) = entry.file_name().to_str() {
                    stopped.push(SignalledEngine {
                        id: id.to_string(),
                        pid,
                    });
                }
            }
        }
    }
    stopped
}

fn read_engine_pid(workspace_dir: &Path) -> Option<i32> {
    lucidos_owner_lock::engine::live_pid(workspace_dir).and_then(|pid| i32::try_from(pid).ok())
}

const HEADER_DEVICE_ID: &str = "x-lucidos-device-id";

const RESTART_INTENT_PATH: &str = "/api/v1/internal/restart-intent";

const DEFAULT_ENGINE_PROTO: &str = "https";

const RESTART_INTENT_TIMEOUT: Duration = Duration::from_millis(500);

const RESTART_INTENT_BUDGET: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestartIntentTarget {
    slug: String,
    port: u16,
    scheme: String,
    device_id: String,
}

fn parse_engine_ports(contents: &str) -> Option<(u16, String)> {
    let mut port = None;
    let mut proto = None;
    for line in contents.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "API_PORT" => port = value.trim().parse::<u16>().ok().filter(|p| *p != 0),
            "PROTO" => proto = Some(value.trim().to_string()).filter(|p| !p.is_empty()),
            _ => {}
        }
    }
    Some((
        port?,
        proto.unwrap_or_else(|| DEFAULT_ENGINE_PROTO.to_string()),
    ))
}

fn is_header_safe(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| (b'!'..=b'~').contains(&b))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartIntentSkip {
    NoPort,
    NoDevice,
}

impl RestartIntentSkip {
    fn reason(self) -> &'static str {
        match self {
            Self::NoPort => "its ports file names no engine port",
            Self::NoDevice => "this client holds no device id for it",
        }
    }
}

fn restart_intent_target(
    slug: &str,
    ports: Option<&str>,
    device_id: Option<&str>,
) -> Result<RestartIntentTarget, RestartIntentSkip> {
    let (port, scheme) = ports
        .and_then(parse_engine_ports)
        .ok_or(RestartIntentSkip::NoPort)?;
    let device_id = device_id
        .map(str::trim)
        .filter(|d| is_header_safe(d))
        .ok_or(RestartIntentSkip::NoDevice)?;
    Ok(RestartIntentTarget {
        slug: slug.to_string(),
        port,
        scheme,
        device_id: device_id.to_string(),
    })
}

fn restart_intent_request(
    client: &reqwest::blocking::Client,
    target: &RestartIntentTarget,
) -> reqwest::blocking::RequestBuilder {
    client
        .post(format!(
            "{}://127.0.0.1:{}{RESTART_INTENT_PATH}",
            target.scheme, target.port
        ))
        .header(HEADER_DEVICE_ID, &target.device_id)
        .body("")
        .timeout(RESTART_INTENT_TIMEOUT)
}

fn post_restart_intent(target: &RestartIntentTarget) -> bool {
    let Some(client) = loopback_client() else {
        return false;
    };
    restart_intent_request(&client, target)
        .send()
        .is_ok_and(|response| response.status() == reqwest::StatusCode::NO_CONTENT)
}

fn announce_to_engines(
    targets: &[RestartIntentTarget],
    deadline: Instant,
    mut post: impl FnMut(&RestartIntentTarget) -> bool,
) -> usize {
    let mut told = 0;
    for (i, target) in targets.iter().enumerate() {
        if Instant::now() >= deadline {
            eprintln!(
                "[desktop] restart intent: out of time, {} workspace(s) not told",
                targets.len() - i
            );
            break;
        }
        if post(target) {
            told += 1;
        } else {
            eprintln!(
                "[desktop] restart intent: '{}' did not acknowledge, so its threads will \
                 read this restart as a crash",
                target.slug
            );
        }
    }
    told
}

fn engine_pid_is_live(workspace_dir: &Path) -> bool {
    let Some(pid) = read_engine_pid(workspace_dir) else {
        return false;
    };
    #[cfg(unix)]
    {
        !process_is_gone(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn restart_intent_targets(app_data: &Path) -> Vec<RestartIntentTarget> {
    let devices = crate::device_id_store::device_ids_by_workspace(app_data);
    let Ok(entries) = std::fs::read_dir(app_data.join("workspaces")) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for entry in entries.flatten() {
        let Some(slug) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !engine_pid_is_live(&entry.path()) {
            continue;
        }
        let ports = std::fs::read_to_string(entry.path().join(".lucidos/ports")).ok();
        let device_id = devices.get(&slug).map(String::as_str);
        match restart_intent_target(&slug, ports.as_deref(), device_id) {
            Ok(target) => targets.push(target),
            Err(skip) => eprintln!(
                "[desktop] restart intent: not telling '{}', {}",
                slug,
                skip.reason()
            ),
        }
    }
    targets
}

fn announce_restart_intent(app_data: &Path) -> usize {
    let targets = restart_intent_targets(app_data);
    if targets.is_empty() {
        return 0;
    }
    announce_to_engines(
        &targets,
        Instant::now() + RESTART_INTENT_BUDGET,
        post_restart_intent,
    )
}

fn announce_restart_intent_from_env() -> usize {
    match app_data_dir_from_env() {
        Ok(app_data) => announce_restart_intent(&app_data),
        Err(e) => {
            eprintln!("[desktop] cannot resolve app-data to announce the restart intent: {e}");
            0
        }
    }
}

const NEXT_BOOT_FILE: &str = ".next-boot.json";

fn next_boot_path(app_data: &Path) -> PathBuf {
    app_data.join(NEXT_BOOT_FILE)
}

const NEXT_BOOT_QUIT: &str = "{\"quit\":true}";

fn record_workspaces_to_restore(app_data: &Path, ids: &[String]) {
    let path = next_boot_path(app_data);
    if quit_was_declared(&path) {
        return;
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

fn quit_was_declared(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|record| record.get("quit")?.as_bool())
        .unwrap_or(false)
}

fn declare_quit_intent(app_data: &Path) {
    if let Err(e) = std::fs::write(next_boot_path(app_data), NEXT_BOOT_QUIT) {
        eprintln!("[service] could not record the quit intent: {e}");
    }
}

fn clear_next_boot_record(app_data: &Path) {
    match std::fs::remove_file(next_boot_path(app_data)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("[service] could not clear the next-boot record: {e}"),
    }
}

fn resource_dir_from_exe() -> io::Result<PathBuf> {
    resource_dir_for_exe(&std::env::current_exe()?)
}

fn resource_dir_for_exe(exe: &Path) -> io::Result<PathBuf> {
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
    system_widgets: PathBuf,
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
        system_widgets: resources.join(SYSTEM_WIDGETS_RESOURCE_NAME),
        pg_bin: postgres.join("bin"),
        pg_lib: postgres.join("lib"),
    }
}

pub fn app_data_dir_from_env() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support")
        .join(BUNDLE_IDENTIFIER))
}

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
    let _ = lucidos_home::ensure_storage_root(app_data)
        .and_then(|()| std::fs::create_dir_all(app_data.join("config")));
    let _ = std::fs::write(&cfg, DEFAULT_ENGINE_PORT.to_string());
    DEFAULT_ENGINE_PORT
}

pub fn engine_port() -> u16 {
    app_data_dir_from_env()
        .map(|d| resolve_engine_port(&d))
        .unwrap_or(DEFAULT_ENGINE_PORT)
}

pub fn gateway_url(port: u16) -> String {
    format!("https://localhost:{port}")
}

static LAUNCH_TARGET: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
pub fn set_launch_target(url: String) {
    *LAUNCH_TARGET.lock().unwrap() = Some(url);
}

fn take_launch_target() -> Option<String> {
    LAUNCH_TARGET.lock().unwrap().take()
}

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

pub(crate) fn gateway_capability(origin: &str) -> tauri::ipc::CapabilityBuilder {
    GATEWAY_PERMISSIONS.iter().fold(
        tauri::ipc::CapabilityBuilder::new("gateway")
            .local(false)
            .remote(origin.to_string())
            .webviews(["main", "window-*"]),
        |capability, permission| capability.permission(*permission),
    )
}

pub(crate) fn handoff_origin(port: u16) -> String {
    format!("http://localhost:{port}")
}

fn first_navigation(url: &str, port: u16, handoff_done: bool) -> String {
    match url.strip_prefix(&gateway_url(port)) {
        Some(rest) if !handoff_done => format!("{}{rest}", handoff_origin(port)),
        _ => url.to_string(),
    }
}

pub(crate) fn handoff_capability(origin: &str) -> tauri::ipc::CapabilityBuilder {
    tauri::ipc::CapabilityBuilder::new("outbox-handoff")
        .local(false)
        .remote(origin.to_string())
        .webviews(["main"])
        .permission("allow-outbox-handoff-store")
}

pub fn launch(app: &AppHandle, nudge_rx: std::sync::mpsc::Receiver<()>) {
    if tauri::is_dev() {
        drop(nudge_rx);
        crate::dev_window::launch(app);
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        std::thread::spawn(move || {
            let port = engine_port();
            let mut last: Option<u64> = None;
            loop {
                if let Some(total) = fetch_unread_total(port) {
                    if last != Some(total) {
                        let h = handle.clone();
                        let applied = handle.run_on_main_thread(move || {
                            crate::activation::apply_unread_indicator(&h, total);
                        });
                        match applied {
                            Ok(()) => last = Some(total),
                            Err(e) => eprintln!("[Tauri] unread indicator not applied: {e}"),
                        }
                    }
                }
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
    drop(nudge_rx);

    crate::update_relay::start(app.clone(), engine_port());

    let handle = app.clone();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        refuse_unstable_bundle_location(&handle);
        let app_data = match app_data_dir_from_env() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[desktop] cannot resolve app_data_dir: {e}");
                return;
            }
        };
        let port = resolve_engine_port(&app_data);

        if let Err(e) = handle.add_capability(gateway_capability(&gateway_url(port))) {
            eprintln!(
                "[desktop] FAILED to register the gateway ACL capability for {} — every Tauri IPC \
                 call from the window will be rejected by the ACL: {e}",
                gateway_url(port)
            );
        }
        if let Err(e) = handle.add_capability(handoff_capability(&handoff_origin(port))) {
            eprintln!(
                "[desktop] FAILED to register the outbox hand-off ACL capability for {}: {e}",
                handoff_origin(port)
            );
        }

        let status = handle.state::<StartupStatus>();
        let tail = ServiceLogTail::start(&app_data.join("logs").join(SERVICE_ERR_LOG));
        let watch = ServiceWatch {
            tail: &tail,
            status: &status,
        };
        loop {
            status.enter(StartupPhase::EnsuringService);
            match ensure_service_installed_and_running(&app_data) {
                Ok(()) => status.clear_failure(),
                Err(e) => {
                    crate::client_log::record_lifecycle(
                        crate::client_log::Lifecycle::ServiceStartFailed,
                    );
                    eprintln!("[desktop] ensure service running failed: {e}");
                    status.note_failure(format!("Could not start the background service: {e}."));
                }
            }
            status.enter(StartupPhase::WaitingForGateway);
            if wait_for_health(port, HEALTH_ENSURE_CYCLE, &watch) {
                crate::client_log::record_lifecycle(
                    crate::client_log::Lifecycle::GatewayHealthReached,
                );
                break;
            }
            crate::client_log::record_lifecycle(crate::client_log::Lifecycle::GatewayHealthRetry);
            eprintln!(
                "[desktop] gateway not healthy yet on port {port}; re-ensuring service and retrying"
            );
        }

        let origin = gateway_url(port);
        handle
            .state::<crate::loopback_trust::LoopbackTrust>()
            .wait_until_trusted(crate::loopback_trust::check_loopback_trust);
        let handoff_done = crate::outbox_handoff::handoff_done(&app_data);

        let restore = crate::window_persist::resolve_window_session_plan();
        let mut plan = launch_plan(take_launch_target(), restore, &origin);
        navigate_main_window(&handle, &first_navigation(&plan.main, port, handoff_done));
        if let Some(url) = take_launch_target() {
            plan = launch_plan(Some(url), restore, &origin);
            navigate_main_window(&handle, &first_navigation(&plan.main, port, handoff_done));
        }
        announce_install_conflict(&handle, &app_data, port);
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

#[cfg(target_os = "macos")]
fn refuse_unstable_bundle_location(app: &AppHandle) {
    use crate::bundle_location;

    let Ok(bundle) = bundle_location::bundle_path() else {
        return;
    };
    let Some(body) = bundle_location::launch_refusal(&bundle_location::current(&bundle)) else {
        return;
    };
    eprintln!(
        "[desktop] refusing to run from {}: {body}",
        bundle.display()
    );
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    app.dialog()
        .message(body)
        .title("Move Lucidos to Applications")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCustom("Quit".to_string()))
        .blocking_show();
    std::process::exit(0);
}

pub fn leftover_installs(app_data: &Path) -> Vec<String> {
    let inventory = lucidos_installs::scan_this_machine(Some(app_data.to_path_buf()));
    lucidos_installs::leftovers_report(&inventory, |i| i.running_here)
}

fn announce_install_conflict(app: &AppHandle, app_data: &Path, port: u16) {
    let Some(notice) =
        crate::install_preflight::take_notice(app_data, port, crate::LUCIDOS_RELEASE)
    else {
        return;
    };
    eprintln!(
        "[desktop] install conflict on port {port}:\n{}",
        notice.body
    );
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    app.dialog()
        .message(notice.body)
        .title("Another Lucidos install is in the way")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::Ok)
        .show(|_| {});
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlannedWindow {
    pub url: String,
    pub label: Option<String>,
    pub frame: Option<crate::window_restore::RememberedFrame>,
}

fn planned_window(origin: &str, window: crate::window_session::RestoredWindow) -> PlannedWindow {
    PlannedWindow {
        url: crate::window_target::workspace_url(origin, &window.workspace),
        label: window
            .label
            .filter(|label| label != crate::app_window::MAIN_WINDOW_LABEL),
        frame: window.frame,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LaunchPlan {
    pub main: String,
    pub extra: Vec<PlannedWindow>,
}

pub(crate) fn launch_plan(
    tap: Option<String>,
    restore: &[crate::window_session::RestoredWindow],
    origin: &str,
) -> LaunchPlan {
    let tapped = tap
        .as_deref()
        .and_then(crate::window_target::window_workspace);
    let skipped = tapped.and_then(|id| restore.iter().position(|w| w.workspace == id));
    let mut wanted: Vec<(usize, crate::window_session::RestoredWindow)> = restore
        .iter()
        .cloned()
        .enumerate()
        .filter(|(index, _)| Some(*index) != skipped)
        .collect();

    let main = match tap {
        Some(url) => {
            if let Some((0, window)) = wanted
                .first_mut()
                .filter(|(_, window)| window.main_may_take())
            {
                window.frame = None;
            }
            url
        }
        None if wanted
            .first()
            .is_some_and(|(_, window)| window.main_may_take()) =>
        {
            let (_, window) = wanted.remove(0);
            crate::window_target::workspace_url(origin, &window.workspace)
        }
        None => origin.to_string(),
    };
    LaunchPlan {
        main,
        extra: wanted
            .into_iter()
            .map(|(_, window)| planned_window(origin, window))
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveWindow {
    pub label: String,
    pub url: String,
    pub visible: bool,
    pub minimized: bool,
    pub last_focused: Option<u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ReopenPlan {
    pub show: Vec<String>,
    pub navigate_main: Option<PlannedWindow>,
    pub build: Vec<PlannedWindow>,
    pub front: Option<String>,
}

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
    let booting = main.is_some_and(|w| !crate::window_target::window_is_navigated(&w.url));

    let mut owed: Vec<crate::window_session::RestoredWindow> = Vec::new();
    let mut recorded_main = None;
    if !booting {
        let served: Vec<&str> = live
            .iter()
            .filter_map(|w| crate::window_target::window_workspace(&w.url))
            .collect();
        for window in crate::window_session::restore_plan(session, true) {
            match window.label.as_deref() {
                Some(crate::app_window::MAIN_WINDOW_LABEL) if main.is_some() => {
                    recorded_main = Some(window);
                }
                Some(label) if live.iter().any(|w| w.label == label) => {}
                None if served.contains(&window.workspace.as_str()) => {}
                _ => owed.push(window),
            }
        }
    }

    let adrift = main.is_some_and(|w| crate::window_target::window_workspace(&w.url).is_none());
    let navigate_main = adrift
        .then(|| {
            recorded_main.or_else(|| {
                let unlabelled = owed.iter().position(|window| window.main_may_take())?;
                Some(owed.remove(unlabelled))
            })
        })
        .flatten()
        .map(|window| PlannedWindow {
            url: crate::window_target::workspace_url(origin, &window.workspace),
            label: None,
            frame: window.frame,
        });

    ReopenPlan {
        front: main
            .map(|w| w.label.clone())
            .or_else(|| show.first().cloned()),
        show,
        navigate_main,
        build: owed
            .into_iter()
            .map(|window| planned_window(origin, window))
            .collect(),
    }
}

pub(crate) fn navigate_main_window(app: &AppHandle, url: &str) {
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

struct ServiceWatch<'a> {
    tail: &'a ServiceLogTail,
    status: &'a StartupStatus,
}

fn wait_for_health(port: u16, timeout: Duration, watch: &ServiceWatch) -> bool {
    let deadline = Instant::now() + timeout;
    let mut next_read = Instant::now();
    while Instant::now() < deadline {
        if http_ok(port, GATEWAY_HEALTH_PATH) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayStart {
    Healthy,
    ChildExited,
    TimedOut,
}

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

fn await_gateway_start(port: u16, timeout: Duration, gateway: &mut Child) -> GatewayStart {
    let deadline = Instant::now() + timeout;
    loop {
        let exited = !matches!(gateway.try_wait(), Ok(None));
        if let Some(outcome) = gateway_start_poll(
            http_ok(port, GATEWAY_HEALTH_PATH),
            exited,
            Instant::now() >= deadline,
        ) {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn service_boot_failed(reason: impl std::fmt::Display) -> i32 {
    eprintln!("[service] {BOOT_FAILED_MARKER} {reason}");
    1
}

pub fn run_service() -> i32 {
    eprintln!("{SERVICE_BOOT_MARKER} (pid {})", std::process::id());

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

    if http_ok(port, GATEWAY_HEALTH_PATH) {
        eprintln!(
            "[service] port {port} is already served by a gateway this service did not spawn; \
             the one below will fail to bind. See Settings, System, Overview for the installs."
        );
    }

    let mut svc = match spawn_gateway(&resources, &app_data, port) {
        Ok(svc) => svc,
        Err(e) => {
            return service_boot_failed(format!("could not start the workspace gateway: {e}"))
        }
    };
    match await_gateway_start(port, ENGINE_HEALTH_TIMEOUT, &mut svc.gateway) {
        GatewayStart::Healthy => {}
        GatewayStart::ChildExited => {
            let outcome = service_boot_failed(format!(
                "the gateway exited before answering on port {port}; see its own reason above"
            ));
            svc.shutdown(&resources, &app_data);
            return outcome;
        }
        GatewayStart::TimedOut => {
            let outcome = service_boot_failed(format!(
                "the gateway did not become healthy on port {port} within {}s",
                ENGINE_HEALTH_TIMEOUT.as_secs()
            ));
            svc.shutdown(&resources, &app_data);
            return outcome;
        }
    }
    eprintln!("[service] gateway healthy on port {port}; supervising");

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

pub fn run_disposable_service(args: Vec<std::ffi::OsString>) -> i32 {
    match disposable_service(args) {
        Ok(code) => code,
        Err(e) => service_boot_failed(e),
    }
}

fn disposable_service(args: Vec<std::ffi::OsString>) -> io::Result<i32> {
    if args.len() != 3
        || (args[0] != "--disposable-service" && args[0] != "--disposable-supervisor")
    {
        return Err(io::Error::other(
            "expected --disposable-service <new-root> <port>",
        ));
    }
    let port = args[2]
        .to_str()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .ok_or_else(|| io::Error::other("invalid disposable port"))?;
    let parent = unsafe { libc::getppid() };
    if parent <= 1 {
        return Err(io::Error::other("disposable controller is absent"));
    }
    install_stop_handlers();
    if args[0] == "--disposable-service" {
        let scope = lucidos_home::disposable::Scope::create(Path::new(&args[1]))?;
        let mut supervisor = scope
            .command(&std::env::current_exe()?)
            .arg("--disposable-supervisor")
            .arg(scope.root())
            .arg(port.to_string())
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = supervisor.try_wait()? {
                return Ok(status.code().unwrap_or(1));
            }
            if SERVICE_STOP.load(Ordering::SeqCst)
                || unsafe { libc::getppid() } != parent
                || started.elapsed() >= lucidos_home::disposable::LIFETIME
            {
                drop(supervisor.stdin.take());
                return Ok(supervisor.wait()?.code().unwrap_or(1));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let scope = lucidos_home::disposable::Scope::open(Path::new(&args[1]))?;
    scope.claim_supervisor()?;
    let resources = resource_dir_from_exe()?;
    if unsafe { libc::setsid() } == -1 {
        return Err(io::Error::last_os_error());
    }
    let controller_closed = watch_disposable_controller();
    let mut svc = spawn_gateway_in_scope(&resources, &scope.app_data(), port, Some(&scope))?;
    let recorded = match std::fs::write(
        scope.root().join("gateway.pid"),
        svc.gateway.id().to_string(),
    ) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("[service] cannot record disposable gateway: {e}");
            false
        }
    };
    let result = supervise_disposable_group(
        &mut svc.gateway,
        parent,
        lucidos_home::disposable::LIFETIME,
        || {
            !recorded
                || SERVICE_STOP.load(Ordering::SeqCst)
                || controller_closed.load(Ordering::SeqCst)
        },
    );
    let bundle = bundled_resources(&resources);
    let data = scope.app_data().join("pgdata");
    if data.join("postmaster.pid").exists() {
        let status = embedded_pg_stop_command(&bundle.pg_bin, &bundle.pg_lib, &data)
            .args(["-t", "10"])
            .status()?;
        if !status.success() {
            let status = embedded_pg_stop_command(&bundle.pg_bin, &bundle.pg_lib, &data)
                .args(["-m", "immediate", "-t", "10"])
                .status()?;
            if !status.success() {
                return Err(io::Error::other("disposable Postgres did not stop"));
            }
        }
    }
    Ok(result)
}

fn watch_disposable_controller() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    let controller_closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let closed = controller_closed.clone();
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        let _ = std::io::stdin().read(&mut byte);
        closed.store(true, Ordering::SeqCst);
    });
    controller_closed
}

fn supervise_disposable_group(
    gateway: &mut Child,
    parent: i32,
    lifetime: Duration,
    canceled: impl Fn() -> bool,
) -> i32 {
    let started = Instant::now();
    let mut stop_code = 0;
    let exited = loop {
        if canceled() || unsafe { libc::getppid() } != parent {
            break false;
        }
        if started.elapsed() > lifetime {
            stop_code = 1;
            break false;
        }
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let waited = unsafe {
            libc::waitid(
                libc::P_PID,
                gateway.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if waited != 0 {
            eprintln!(
                "[service] disposable gateway wait failed: {}",
                io::Error::last_os_error()
            );
            return 1;
        }
        if info.si_signo != 0 {
            break true;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let group = -(gateway.id() as i32);
    unsafe {
        libc::kill(gateway.id() as i32, libc::SIGUSR1);
        libc::kill(group, libc::SIGTERM);
    }
    std::thread::sleep(lucidos_home::disposable::STOP_GRACE);
    unsafe {
        libc::kill(group, libc::SIGKILL);
    }
    match gateway.wait() {
        Ok(status) if exited => status.code().unwrap_or(1),
        Ok(_) => stop_code,
        Err(e) => {
            eprintln!("[service] disposable gateway reap failed: {e}");
            1
        }
    }
}

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

fn ensure_service_installed_and_running(app_data: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;

    #[cfg(target_os = "macos")]
    ensure_login_agent_installed(&exe, app_data);

    let changed = install_or_update_service_plist(&exe, app_data)?;

    if changed && is_service_loaded() {
        let _ = bootout_service();
        bootstrap_service()?;
    } else if !is_service_loaded() {
        bootstrap_service()?;
    } else {
        let _ = kickstart_service(false);
    }
    Ok(())
}

fn agent_plist_path(label: &str) -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist")))
}

fn service_plist_path() -> io::Result<PathBuf> {
    agent_plist_path(SERVICE_AGENT_LABEL)
}

#[cfg(target_os = "macos")]
fn login_plist_path() -> io::Result<PathBuf> {
    agent_plist_path(LOGIN_AGENT_LABEL)
}

fn xml_escape_str(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_escape(p: &Path) -> String {
    xml_escape_str(&p.to_string_lossy())
}

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
    <key>ExitTimeOut</key>
    <integer>{exit_timeout}</integer>
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
        exit_timeout = LAUNCHD_EXIT_TIMEOUT.as_secs(),
        exe = xml_escape(exe),
        out = xml_escape(&out),
        err = xml_escape(&err),
    )
}

fn write_plist_if_changed(path: &Path, app_data: &Path, desired: &str) -> io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    lucidos_home::ensure_storage_root(app_data)?;
    std::fs::create_dir_all(app_data.join("logs"))?;
    if std::fs::read_to_string(path).ok().as_deref() == Some(desired) {
        return Ok(false);
    }
    std::fs::write(path, desired)?;
    Ok(true)
}

fn install_or_update_service_plist(exe: &Path, app_data: &Path) -> io::Result<bool> {
    let desired = desired_service_plist(exe, app_data);
    write_plist_if_changed(&service_plist_path()?, app_data, &desired)
}

#[cfg(target_os = "macos")]
const LOGIN_OPEN_ATTEMPTS: u32 = 10;

#[cfg(target_os = "macos")]
const LOGIN_OPEN_RETRY_SECONDS: u32 = 3;

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

#[cfg(target_os = "macos")]
fn desired_login_plist_for_exe(exe: &Path, app_data: &Path) -> Result<Option<String>, String> {
    match app_bundle_root_from_exe(exe) {
        Some(bundle) => desired_login_plist(&bundle, app_data).map(Some),
        None => Ok(None),
    }
}

#[cfg(target_os = "macos")]
fn ensure_login_agent_installed(exe: &Path, app_data: &Path) {
    let desired = match desired_login_plist_for_exe(exe, app_data) {
        Ok(Some(desired)) => desired,
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
        Ok(false) => {}
        Ok(true) => {
            let target = login_target();
            if is_job_loaded(&target) {
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

fn launchd_domain() -> String {
    format!("gui/{}", current_uid())
}

fn launchd_target(label: &str) -> String {
    format!("gui/{}/{}", current_uid(), label)
}

fn service_target() -> String {
    launchd_target(SERVICE_AGENT_LABEL)
}

#[cfg(target_os = "macos")]
fn login_target() -> String {
    launchd_target(LOGIN_AGENT_LABEL)
}

fn launchctl(args: &[&str]) -> io::Result<std::process::Output> {
    Command::new("launchctl").args(args).output()
}

fn is_job_loaded(target: &str) -> bool {
    launchctl(&["print", target])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_service_loaded() -> bool {
    is_job_loaded(&service_target())
}

fn bootstrap_job(plist: &Path, target: &str) -> io::Result<()> {
    let plist_s = plist.to_string_lossy().to_string();
    let out = launchctl(&["bootstrap", &launchd_domain(), &plist_s])?;
    if !out.status.success() {
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

pub fn restart_service() -> Result<(), String> {
    if tauri::is_dev() {
        return Err("Gateway service restart is only available in a packaged build".to_string());
    }
    let announced = announce_restart_intent_from_env();
    kickstart_service(true).map_err(|e| {
        if announced > 0 {
            eprintln!(
                "[desktop] restart did not start, but {announced} engine(s) already hold a \
                 restart intent that their next teardown will spend"
            );
        }
        e.to_string()
    })
}

pub fn stop_service() {
    if tauri::is_dev() {
        return;
    }
    let mut announced = 0;
    let app_data = match app_data_dir_from_env() {
        Ok(app_data) => {
            declare_quit_intent(&app_data);
            announced = announce_restart_intent(&app_data);
            Some(app_data)
        }
        Err(e) => {
            eprintln!("[desktop] cannot resolve app-data for the quit intent: {e}");
            None
        }
    };
    if let Err(e) = bootout_service() {
        eprintln!("[desktop] stop service failed: {e}");
        if let Some(app_data) = &app_data {
            clear_next_boot_record(app_data);
        }
        if announced > 0 {
            eprintln!(
                "[desktop] stop did not happen, but {announced} engine(s) already hold a \
                 restart intent that their next teardown will spend"
            );
        }
    }
}

#[cfg(target_os = "macos")]
const RELAUNCH_POLL_SECONDS: &str = "0.1";

#[cfg(target_os = "macos")]
const RELAUNCH_WAIT_PROBES: u32 = 3000;

#[cfg(target_os = "macos")]
const RELAUNCH_OPEN_ATTEMPTS: u32 = 10;

#[cfg(target_os = "macos")]
const RELAUNCH_OPEN_RETRY_SECONDS: u32 = 2;

pub fn relaunch_args() -> Vec<std::ffi::OsString> {
    strip_login_flag(std::env::args_os().skip(1))
}

fn strip_login_flag(args: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    args.into_iter()
        .filter(|a| a != std::ffi::OsStr::new(LOGIN_FLAG))
        .collect()
}

#[cfg(target_os = "macos")]
pub fn schedule_relaunch_after_exit() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("resolve this executable: {e}"))?;
    let bundle = app_bundle_root_from_exe(&exe)
        .ok_or_else(|| format!("{} is not inside a .app bundle", exe.display()))?;
    let args = relaunch_args();
    let script = relaunch_watcher_script(
        std::process::id(),
        &bundle,
        &relaunched_exe_forms(&exe),
        &args,
    )?;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("exec 2>&1; {script}"))
        .stdout(
            crate::client_log::relaunch_output(&exe)
                .map_err(|e| format!("spawn relaunch diagnostic capture: {e}"))?,
        )
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn the relaunch watcher: {e}"))?;
    crate::client_log::record_lifecycle(crate::client_log::Lifecycle::RelaunchScheduled);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn schedule_relaunch_after_exit() -> Result<(), String> {
    Err("a LaunchServices relaunch is macOS-only".to_string())
}

#[cfg(target_os = "macos")]
fn relaunched_exe_forms(exe: &Path) -> Vec<PathBuf> {
    let mut forms = vec![exe.to_path_buf()];
    if let Ok(real) = std::fs::canonicalize(exe) {
        if real != *exe {
            forms.push(real);
        }
    }
    forms
}

#[cfg(target_os = "macos")]
fn relaunch_watcher_script(
    pid: u32,
    bundle: &Path,
    exes: &[PathBuf],
    args: &[std::ffi::OsString],
) -> Result<String, String> {
    let bundle = bundle
        .to_str()
        .ok_or_else(|| format!("bundle path is not valid UTF-8: {}", bundle.display()))?;
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .ok_or_else(|| "an argument is not valid UTF-8".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut launch = format!("/usr/bin/open -a {}", sh_quote(bundle));
    if !args.is_empty() {
        launch.push_str(" --args");
        for arg in &args {
            launch.push(' ');
            launch.push_str(&sh_quote(arg));
        }
    }
    let running = sh_quote(&running_pattern(exes, &args)?);
    let reporter = sh_quote(&exes[0].to_string_lossy());
    let stayed_running = crate::RelaunchOutcome::PreviousClientStillRunning.arg();
    let observed = crate::RelaunchOutcome::ClientObserved.arg();
    let exhausted = crate::RelaunchOutcome::AttemptsExhausted.arg();
    let stuck = sh_quote(&format!(
        "[relaunch] pid {pid} never exited, so Lucidos was not relaunched"
    ));
    let refused = sh_quote(&format!("[relaunch] LaunchServices refused {bundle}"));
    let gave_up = sh_quote(&format!(
        "[relaunch] gave up on {bundle} after {RELAUNCH_OPEN_ATTEMPTS} attempts; \
         Lucidos must be started by hand"
    ));
    Ok(format!(
        "i=0; while [ $i -lt {RELAUNCH_WAIT_PROBES} ] && kill -0 {pid} 2>/dev/null; \
         do sleep {RELAUNCH_POLL_SECONDS}; i=$((i+1)); done; \
         if kill -0 {pid} 2>/dev/null; then \
         echo {stuck} >&2; {reporter} {stayed_running}; exit 1; fi; \
         a=0; while [ $a -lt {RELAUNCH_OPEN_ATTEMPTS} ]; do \
         {launch} || echo {refused} >&2; \
         sleep {RELAUNCH_OPEN_RETRY_SECONDS}; \
         /usr/bin/pgrep -f {running} >/dev/null 2>&1 && {{ {reporter} {observed}; exit 0; }}; \
         a=$((a+1)); done; \
         echo {gave_up} >&2; {reporter} {exhausted}; exit 1"
    ))
}

#[cfg(target_os = "macos")]
fn running_pattern(exes: &[PathBuf], args: &[&str]) -> Result<String, String> {
    let escaped = exes
        .iter()
        .map(|exe| {
            exe.to_str()
                .map(ere_escape)
                .ok_or_else(|| format!("executable path is not valid UTF-8: {}", exe.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let head = match escaped.as_slice() {
        [] => return Err("no executable path to watch for".to_string()),
        [one] => format!("^{one}"),
        many => format!("^({})", many.join("|")),
    };
    let tail: String = args.iter().map(|a| format!(" {}", ere_escape(a))).collect();
    Ok(format!("{head}{tail}$"))
}

#[cfg(target_os = "macos")]
fn ere_escape(s: &str) -> String {
    const META: &[char] = &[
        '\\', '.', '[', ']', '(', ')', '{', '}', '*', '+', '?', '^', '$', '|',
    ];
    s.chars()
        .flat_map(|c| {
            META.contains(&c)
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(target_os = "macos")]
const LAUNCHD_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(target_os = "macos")]
const ENGINE_EXIT_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(unix)]
const TEARDOWN_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeardownWait {
    Gone,
    StillThere,
}

#[cfg(unix)]
fn teardown_poll(gone: bool, out_of_time: bool) -> Option<TeardownWait> {
    if gone {
        return Some(TeardownWait::Gone);
    }
    if out_of_time {
        return Some(TeardownWait::StillThere);
    }
    None
}

#[cfg(unix)]
fn wait_until_gone(timeout: Duration, mut gone: impl FnMut() -> bool) -> TeardownWait {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(outcome) = teardown_poll(gone(), Instant::now() >= deadline) {
            return outcome;
        }
        std::thread::sleep(TEARDOWN_POLL_INTERVAL);
    }
}

#[cfg(target_os = "macos")]
fn launchd_domain_reachable() -> bool {
    launchctl(&["print", &launchd_domain()])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn service_has_left_the_domain() -> bool {
    !is_service_loaded() && launchd_domain_reachable()
}

#[cfg(target_os = "macos")]
fn wait_for_service_to_unload() -> TeardownWait {
    wait_until_gone(LAUNCHD_TEARDOWN_TIMEOUT, service_has_left_the_domain)
}

#[cfg(unix)]
fn process_is_gone(pid: i32) -> bool {
    // SAFETY: signal 0 delivers nothing. It only asks whether the pid exists
    // and whether this process could signal it.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return false;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(unix)]
fn wait_for_engines_to_exit(
    engines: &[SignalledEngine],
    timeout: Duration,
) -> Vec<SignalledEngine> {
    let mut alive = engines.to_vec();
    let verdict = wait_until_gone(timeout, || {
        alive.retain(|e| !process_is_gone(e.pid));
        alive.is_empty()
    });
    match verdict {
        TeardownWait::Gone => Vec::new(),
        TeardownWait::StillThere => alive,
    }
}

#[cfg(target_os = "macos")]
fn delete_may_proceed(service: TeardownWait, live_engines: &[String]) -> Result<(), String> {
    let mut blockers: Vec<String> = Vec::new();
    if service == TeardownWait::StillThere {
        blockers.push("the background service".to_string());
    }
    blockers.extend(live_engines.iter().map(|id| format!("workspace {id}")));
    if blockers.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Lucidos did not confirm a full stop in time. Possibly still running: {}. \
         Nothing was deleted, so you can try the uninstall again.",
        blockers.join(", ")
    ))
}

#[cfg(target_os = "macos")]
pub fn uninstall(app_data: &Path, delete_data: bool) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();

    let signalled = stop_workspace_engines(app_data);

    match resource_dir_from_exe() {
        Ok(resources) => stop_embedded_postgres(&resources, app_data),
        Err(e) => {
            eprintln!("[service] uninstall: cannot resolve resources to stop Postgres: {e}")
        }
    }

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

    let service = if bootout_ok {
        wait_for_service_to_unload()
    } else {
        TeardownWait::StillThere
    };
    let live_engines: Vec<String> = match service {
        TeardownWait::Gone => wait_for_engines_to_exit(&signalled, ENGINE_EXIT_TIMEOUT)
            .into_iter()
            .map(|e| e.id)
            .collect(),
        TeardownWait::StillThere => Vec::new(),
    };
    if let Err(reason) = delete_may_proceed(service, &live_engines) {
        eprintln!("[service] uninstall: refusing to delete anything: {reason}");
        failures.push(reason);
        return Err(failures.join("; "));
    }
    eprintln!("[service] uninstall: the stack is stopped; deleting");

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

    if delete_data {
        if let Err(e) = forget_loopback_ca() {
            eprintln!("[service] uninstall: {e}");
            failures.push(e);
            data_ok = false;
        }
    }

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
            eprintln!(
                "[service] uninstall: not running from a .app bundle; skipping bundle removal"
            );
        }
    }

    if data_ok {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(target_os = "macos")]
fn forget_loopback_ca() -> Result<(), String> {
    use lucidos_loopback_ca::{trust, CaDir};
    let Some(ca) = CaDir::resolve() else {
        return Err("cannot resolve the loopback CA directory: HOME is not set".to_string());
    };
    if trust::forget(&ca).map_err(|e| format!("forget the loopback CA: {e}"))? {
        eprintln!(
            "[service] uninstall: removed the loopback CA at {}",
            ca.path().display()
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall(_app_data: &Path, _delete_data: bool) -> Result<(), String> {
    Err("Uninstall is only supported on macOS".to_string())
}

#[cfg(target_os = "macos")]
fn support_data_paths(home: &Path, app_data: &Path, delete_data: bool) -> Vec<PathBuf> {
    let library = home.join("Library");
    let mut paths = Vec::new();
    if delete_data {
        paths.push(app_data.to_path_buf());
        paths.push(library.join("WebKit").join(BUNDLE_IDENTIFIER));
        paths.push(library.join("WebKit").join("lucidos-app"));
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

pub(crate) fn is_packaged_desktop() -> bool {
    #[cfg(target_os = "macos")]
    {
        !tauri::is_dev()
            && std::env::current_exe()
                .ok()
                .and_then(|exe| app_bundle_root_from_exe(&exe))
                .is_some()
    }
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "macos")]
fn delete_path(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

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

#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn spawn_gateway(resources: &Path, app_data: &Path, port: u16) -> io::Result<GatewayService> {
    spawn_gateway_in_scope(resources, app_data, port, None)
}

fn spawn_gateway_in_scope(
    resources: &Path,
    app_data: &Path,
    port: u16,
    scope: Option<&lucidos_home::disposable::Scope>,
) -> io::Result<GatewayService> {
    let bundle = bundled_resources(resources);
    lucidos_home::ensure_storage_root(app_data)?;

    let fastembed_cache = app_data.join("fastembed");
    if scope.is_none() {
        std::fs::create_dir_all(&fastembed_cache)?;
        std::fs::create_dir_all(app_data.join("config"))?;
    }
    let mut command = match scope {
        Some(scope) => {
            use std::os::unix::process::CommandExt;
            let mut cmd = scope.command(&bundle.gateway_bin);
            cmd.arg("--disposable")
                .arg(scope.root())
                .process_group(0)
                .env("LUCIDOS_GATEWAY_BIND_ADDR", "127.0.0.1")
                .env("LUCIDOS_GATEWAY_ENGINE_LOOPBACK", "1")
                .env("LUCIDOS_HOOK_PORT", "0")
                .env("LUCIDOS_PUBLIC_PORT", "0");
            cmd
        }
        None => Command::new(&bundle.gateway_bin),
    };
    let desktop_data_env = lucidos_diagnostic_log::DESKTOP_GATEWAY_DATA_ENV;
    command.env_remove(desktop_data_env);
    if scope.is_none() && is_packaged_desktop() {
        command.env(desktop_data_env, app_data);
    }
    let gateway = command
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
        .env("LUCIDOS_SYSTEM_WIDGETS_DIR", &bundle.system_widgets)
        .env("FASTEMBED_CACHE_DIR", &fastembed_cache)
        .env("LUCIDOS_BOOT_WITHOUT_PROVIDER", "1")
        .env("LUCIDOS_PACKAGED", "1")
        .spawn()?;
    Ok(GatewayService { gateway })
}

fn loopback_client() -> Option<reqwest::blocking::Client> {
    lucidos_local_token::client::blocking_client_builder()
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|e| eprintln!("[desktop] could not start the loopback client: {e}"))
        .ok()
}

fn http_ok(port: u16, path: &str) -> bool {
    let Some(client) = loopback_client() else {
        return false;
    };
    client
        .get(format!("{}{path}", gateway_url(port)))
        .timeout(Duration::from_secs(2))
        .send()
        .is_ok_and(|response| response.status() == reqwest::StatusCode::OK)
}

pub(crate) fn gateway_body(
    port: u16,
    method: &str,
    path: &str,
    json: Option<&str>,
) -> Option<String> {
    let client = loopback_client()?;
    let method = reqwest::Method::from_bytes(method.as_bytes()).ok()?;
    let mut request = client
        .request(method, format!("{}{path}", gateway_url(port)))
        .timeout(Duration::from_secs(3));
    if let Some(token) = lucidos_local_token::read() {
        let token = crate::client_log::secret(token);
        request = request.header(lucidos_local_token::HEADER_LOCAL_TOKEN, token);
    }
    request = match json {
        Some(body) => request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string()),
        None => request.body(""),
    };
    let response = request.send().ok()?;
    if response.status() != reqwest::StatusCode::OK {
        return None;
    }
    response.text().ok()
}

#[cfg(target_os = "macos")]
fn fetch_unread_total(port: u16) -> Option<u64> {
    let body = gateway_body(port, "GET", "/~/api/v1/control/unread-total", None)?;
    parse_unread_total(&body)
}

#[cfg(target_os = "macos")]
fn parse_unread_total(body: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json.get("total")?.as_u64()
}

#[cfg(test)]
mod tests {
    const DISPOSABLE_FIXTURE: &str = "desktop::tests::disposable_lifecycle_fixture";

    #[test]
    #[ignore = "subprocess fixture for disposable lifetime tests"]
    fn disposable_lifecycle_fixture() {
        use std::os::unix::process::CommandExt;
        let root = std::path::PathBuf::from(std::env::var_os("DISPOSABLE_FIXTURE_ROOT").unwrap());
        let role = std::env::var("DISPOSABLE_FIXTURE_ROLE").unwrap();
        if role == "gateway" {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            std::fs::write(
                root.join("ready"),
                listener.local_addr().unwrap().to_string(),
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_secs(20));
            return;
        }
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", DISPOSABLE_FIXTURE])
            .env(
                "DISPOSABLE_FIXTURE_ROLE",
                if role == "controller" {
                    "supervisor"
                } else {
                    "gateway"
                },
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .process_group(0);
        let mut child = command.spawn().unwrap();
        if role == "controller" {
            if std::env::var_os("DISPOSABLE_FIXTURE_CLOSE").is_some() {
                while !root.join("ready").exists() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                drop(child.stdin.take());
            }
            let _ = child.wait();
        } else {
            let closed = super::watch_disposable_controller();
            super::supervise_disposable_group(
                &mut child,
                unsafe { libc::getppid() },
                std::time::Duration::from_secs(10),
                || closed.load(std::sync::atomic::Ordering::SeqCst),
            );
        }
    }

    #[test]
    fn disposable_supervisor_cleans_up_after_controller_death_and_pipe_close() {
        for kill_controller in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--ignored", "--exact", DISPOSABLE_FIXTURE])
                .env("DISPOSABLE_FIXTURE_ROOT", root.path())
                .env("DISPOSABLE_FIXTURE_ROLE", "controller")
                .stdout(std::process::Stdio::null());
            if !kill_controller {
                command.env("DISPOSABLE_FIXTURE_CLOSE", "1");
            }
            let mut controller = command.spawn().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while !root.path().join("ready").exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if !root.path().join("ready").exists() {
                let _ = controller.kill();
                let _ = controller.wait();
                panic!("disposable fixture did not start");
            }
            let address: std::net::SocketAddr = std::fs::read_to_string(root.path().join("ready"))
                .unwrap()
                .parse()
                .unwrap();
            if kill_controller {
                controller.kill().unwrap();
            }
            while std::net::TcpStream::connect(address).is_ok()
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let closed = std::net::TcpStream::connect(address).is_err();
            let _ = controller.kill();
            let _ = controller.wait();
            assert!(closed, "disposable gateway outlived its controller");
        }
    }

    #[test]
    fn disposable_supervisor_stops_owned_group_on_cancel_timeout_and_exit() {
        use std::os::unix::process::CommandExt;
        for mode in ["cancel", "timeout", "exit"] {
            let mut command = std::process::Command::new("/bin/sh");
            command
                .args(["-c", if mode == "exit" { "true" } else { "sleep 30" }])
                .process_group(0);
            let mut child = command.spawn().unwrap();
            let result = super::supervise_disposable_group(
                &mut child,
                unsafe { libc::getppid() },
                std::time::Duration::from_millis(if mode == "exit" { 5000 } else { 100 }),
                || mode == "cancel",
            );
            assert_eq!(result, if mode == "timeout" { 1 } else { 0 });
            assert!(child.try_wait().unwrap().is_some());
        }
    }

    #[test]
    fn disposable_service_refuses_partial_and_mixed_launch_arguments() {
        for args in [
            vec!["--disposable-service"],
            vec!["--disposable-service", "/tmp/run", "0"],
            vec!["--disposable-service", "--service", "/tmp/run", "5300"],
        ] {
            assert!(super::disposable_service(args.into_iter().map(Into::into).collect()).is_err());
        }
    }

    use super::*;

    #[test]
    fn the_first_launch_goes_through_the_hand_off_page_once() {
        let url = format!("{}/myws/#thread=t", gateway_url(5252));
        assert_eq!(
            first_navigation(&url, 5252, false),
            "http://localhost:5252/myws/#thread=t"
        );
        assert_eq!(first_navigation(&url, 5252, true), url);
        let foreign = "https://example.com/x";
        assert_eq!(first_navigation(foreign, 5252, false), foreign);
    }

    #[test]
    fn the_trust_screen_outranks_the_start_line() {
        let status = StartupStatus::default();
        let trust = crate::loopback_trust::LoopbackTrust::default();
        assert_eq!(status.report(&trust).label, STARTING_LABEL);
        assert_eq!(status.report(&trust).trust_button, None);
        trust.wait_until_trusted(|| lucidos_loopback_ca::trust::TrustStatus::Trusted);
        assert_eq!(status.report(&trust).label, STARTING_LABEL);
    }

    fn remembered(
        x: i64,
        y: i64,
        width: i64,
        height: i64,
    ) -> crate::window_restore::RememberedFrame {
        crate::window_restore::RememberedFrame::unanchored(crate::window_restore::Rect {
            x,
            y,
            width,
            height,
        })
    }

    const ORIGIN: &str = "http://localhost:3210";

    fn planned(url: &str, frame: Option<crate::window_restore::RememberedFrame>) -> PlannedWindow {
        PlannedWindow {
            url: url.to_string(),
            label: None,
            frame,
        }
    }

    fn restored(
        workspace: &str,
        frame: Option<crate::window_restore::RememberedFrame>,
    ) -> crate::window_session::RestoredWindow {
        crate::window_session::RestoredWindow {
            label: None,
            workspace: workspace.to_string(),
            frame,
        }
    }

    fn labelled(
        label: &str,
        workspace: &str,
        frame: Option<crate::window_restore::RememberedFrame>,
    ) -> crate::window_session::RestoredWindow {
        crate::window_session::RestoredWindow {
            label: Some(label.to_string()),
            ..restored(workspace, frame)
        }
    }

    #[test]
    fn an_empty_session_leaves_main_on_the_picker() {
        let plan = launch_plan(None, &[], ORIGIN);
        assert_eq!(plan.main, ORIGIN);
        assert!(plan.extra.is_empty());
    }

    #[test]
    fn every_recorded_workspace_gets_a_window() {
        let plan = launch_plan(
            None,
            &[
                restored("myws", Some(remembered(0, 0, 1200, 800))),
                restored("dev", Some(remembered(10, 20, 900, 700))),
            ],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/myws/");
        assert_eq!(
            plan.extra,
            vec![planned(
                "http://localhost:3210/dev/",
                Some(remembered(10, 20, 900, 700))
            )]
        );
    }

    #[test]
    fn a_workspace_with_no_remembered_frame_still_gets_a_window() {
        let plan = launch_plan(
            None,
            &[restored("myws", None), restored("dev", None)],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/myws/");
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/dev/", None)]
        );
    }

    #[test]
    fn a_notification_tap_wins_the_main_window() {
        let plan = launch_plan(
            Some("http://localhost:3210/tapped/".to_string()),
            &[restored("myws", None)],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/tapped/");
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/myws/", None)]
        );
    }

    #[test]
    fn the_tapped_workspace_is_not_restored_a_second_time() {
        let plan = launch_plan(
            Some("http://localhost:3210/myws/".to_string()),
            &[restored("myws", None), restored("dev", None)],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/myws/");
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/dev/", None)]
        );
    }

    #[test]
    fn a_tap_on_no_workspace_skips_nothing() {
        let plan = launch_plan(
            Some("http://localhost:3210/~/".to_string()),
            &[restored("myws", None)],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/~/");
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/myws/", None)]
        );
    }

    #[test]
    fn a_late_tap_returns_the_workspace_it_displaced_to_the_extras() {
        let restore = [
            restored("myws", Some(remembered(0, 0, 1200, 800))),
            restored("dev", None),
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
                planned("http://localhost:3210/myws/", None),
                planned("http://localhost:3210/dev/", None),
            ],
            "the workspace the tap displaced must still get a window",
        );
    }

    #[test]
    fn a_late_tap_on_a_restored_workspace_does_not_open_it_twice() {
        let restore = [restored("myws", None), restored("dev", None)];
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

    #[test]
    fn a_tap_on_the_first_restored_workspace_costs_no_other_window_its_size() {
        let restore = [
            restored("myws", Some(remembered(0, 0, 1200, 800))),
            restored("dev", Some(remembered(50, 60, 900, 700))),
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
                Some(remembered(50, 60, 900, 700))
            )]
        );
    }

    #[test]
    fn the_restored_windows_follow_the_origin_they_are_given() {
        let plan = launch_plan(
            None,
            &[restored("myws", None), restored("dev", None)],
            "https://box.tail1234.ts.net",
        );
        assert_eq!(plan.main, "https://box.tail1234.ts.net/myws/");
        assert_eq!(
            plan.extra,
            vec![planned("https://box.tail1234.ts.net/dev/", None)]
        );
    }

    #[test]
    fn restored_windows_keep_their_labels() {
        let plan = launch_plan(
            None,
            &[
                labelled("main", "dev", None),
                labelled(WINDOW_A, "dev", None),
            ],
            ORIGIN,
        );
        assert_eq!(plan.main, "http://localhost:3210/dev/");
        assert_eq!(plan.extra.len(), 1);
        assert_eq!(plan.extra[0].label.as_deref(), Some(WINDOW_A));
    }

    #[test]
    fn a_window_displaced_from_main_gets_a_fresh_label() {
        let plan = launch_plan(
            Some("http://localhost:3210/tapped/".to_string()),
            &[labelled("main", "dev", None)],
            ORIGIN,
        );
        assert_eq!(
            plan.extra,
            vec![planned("http://localhost:3210/dev/", None)]
        );
    }

    #[test]
    fn a_tap_skips_one_window_on_its_workspace() {
        let plan = launch_plan(
            Some("http://localhost:3210/dev/".to_string()),
            &[
                labelled("main", "dev", None),
                labelled(WINDOW_A, "dev", None),
            ],
            ORIGIN,
        );
        assert_eq!(plan.extra.len(), 1);
        assert_eq!(plan.extra[0].label.as_deref(), Some(WINDOW_A));
    }

    #[test]
    fn main_never_takes_a_labelled_window_at_launch() {
        let plan = launch_plan(None, &[labelled(WINDOW_A, "dev", None)], ORIGIN);
        assert_eq!(plan.main, ORIGIN);
        assert_eq!(plan.extra.len(), 1);
        assert_eq!(plan.extra[0].label.as_deref(), Some(WINDOW_A));
    }

    fn live(label: &str, url: &str, visible: bool) -> LiveWindow {
        LiveWindow {
            label: label.to_string(),
            url: url.to_string(),
            visible,
            minimized: false,
            last_focused: None,
        }
    }

    fn session(
        open: &[&str],
        geometry: &[(&str, crate::window_restore::RememberedFrame)],
    ) -> crate::window_session::WindowSession {
        crate::window_session::WindowSession {
            open: open
                .iter()
                .map(|id| crate::window_session::OpenWindow {
                    label: None,
                    workspace: id.to_string(),
                    frame: None,
                })
                .collect(),
            geometry: geometry
                .iter()
                .map(|(id, frame)| (id.to_string(), frame.clone()))
                .collect(),
            units: crate::window_session::FrameUnits::LogicalPoints,
        }
    }

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
        assert!(plan.build.is_empty());
        assert_eq!(plan.navigate_main, None);
    }

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

    #[test]
    fn a_reopen_with_nothing_restored_rebuilds_from_the_record() {
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/", false)],
            &session(
                &["myws", "dev"],
                &[
                    ("myws", remembered(0, 0, 1200, 800)),
                    ("dev", remembered(9, 9, 900, 700)),
                ],
            ),
            ORIGIN,
        );
        assert_eq!(
            plan.navigate_main,
            Some(planned(
                "http://localhost:3210/myws/",
                Some(remembered(0, 0, 1200, 800))
            ))
        );
        assert_eq!(
            plan.build,
            vec![planned(
                "http://localhost:3210/dev/",
                Some(remembered(9, 9, 900, 700))
            )]
        );
        assert_eq!(plan.show, vec!["main"]);
        assert_eq!(plan.front.as_deref(), Some("main"));
    }

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

    fn labelled_session(windows: &[(&str, &str)]) -> crate::window_session::WindowSession {
        crate::window_session::WindowSession {
            open: windows
                .iter()
                .map(|(label, id)| crate::window_session::OpenWindow {
                    label: Some(label.to_string()),
                    workspace: id.to_string(),
                    frame: None,
                })
                .collect(),
            ..crate::window_session::WindowSession::default()
        }
    }

    const WINDOW_A: &str = "window-0123456789abcdef0123456789abcdef";
    const WINDOW_B: &str = "window-fedcba9876543210fedcba9876543210";

    #[test]
    fn two_windows_on_one_workspace_are_both_rebuilt_under_their_labels() {
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/~/", false)],
            &labelled_session(&[("main", "dev"), (WINDOW_A, "dev"), (WINDOW_B, "dev")]),
            ORIGIN,
        );
        assert_eq!(
            plan.navigate_main,
            Some(planned("http://localhost:3210/dev/", None))
        );
        let built: Vec<Option<&str>> = plan.build.iter().map(|w| w.label.as_deref()).collect();
        assert_eq!(built, vec![Some(WINDOW_A), Some(WINDOW_B)]);
    }

    #[test]
    fn a_recorded_label_a_live_window_wears_is_never_rebuilt() {
        let plan = reopen_plan(
            &[
                live("main", "http://localhost:3210/myws/", false),
                live(WINDOW_A, "http://localhost:3210/other/", false),
            ],
            &labelled_session(&[("main", "myws"), (WINDOW_A, "dev"), (WINDOW_B, "dev")]),
            ORIGIN,
        );
        assert_eq!(plan.navigate_main, None);
        let built: Vec<Option<&str>> = plan.build.iter().map(|w| w.label.as_deref()).collect();
        assert_eq!(built, vec![Some(WINDOW_B)]);
    }

    #[test]
    fn an_adrift_main_takes_its_own_recorded_window() {
        let mut session = labelled_session(&[(WINDOW_A, "dev")]);
        session.open.push(crate::window_session::OpenWindow {
            label: Some("main".to_string()),
            workspace: "myws".to_string(),
            frame: Some(remembered(1, 2, 1200, 800)),
        });
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/~/", false)],
            &session,
            ORIGIN,
        );
        assert_eq!(
            plan.navigate_main,
            Some(planned(
                "http://localhost:3210/myws/",
                Some(remembered(1, 2, 1200, 800))
            ))
        );
        assert_eq!(plan.build.len(), 1);
        assert_eq!(plan.build[0].label.as_deref(), Some(WINDOW_A));
    }

    #[test]
    fn an_adrift_main_never_takes_a_labelled_window() {
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/~/", true)],
            &labelled_session(&[(WINDOW_A, "dev")]),
            ORIGIN,
        );
        assert_eq!(plan.navigate_main, None);
        assert_eq!(plan.build.len(), 1);
        assert_eq!(plan.build[0].label.as_deref(), Some(WINDOW_A));
    }

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

    #[test]
    fn a_reopen_mid_boot_leaves_the_record_to_the_launch() {
        let plan = reopen_plan(
            &[live("main", "tauri://localhost", false)],
            &session(&["myws", "dev"], &[]),
            ORIGIN,
        );
        assert_eq!(plan.navigate_main, None);
        assert!(plan.build.is_empty());
        assert_eq!(plan.show, vec!["main"]);
        assert_eq!(plan.front.as_deref(), Some("main"));
    }

    #[test]
    fn a_main_on_the_gateway_root_is_past_boot() {
        let plan = reopen_plan(
            &[live("main", "http://localhost:3210/~/", false)],
            &session(&["myws"], &[]),
            ORIGIN,
        );
        assert_eq!(
            plan.navigate_main,
            Some(planned("http://localhost:3210/myws/", None))
        );
    }

    #[test]
    fn a_rebuilt_window_follows_the_origin_it_is_given() {
        let plan = reopen_plan(&[], &session(&["dev"], &[]), "https://box.tail1234.ts.net");
        assert_eq!(
            plan.build,
            vec![planned("https://box.tail1234.ts.net/dev/", None)]
        );
    }

    #[test]
    fn a_dead_gateway_ends_the_wait_immediately() {
        assert_eq!(
            gateway_start_poll(false, true, false),
            Some(GatewayStart::ChildExited)
        );
    }

    #[test]
    fn a_gateway_that_answered_is_healthy_even_if_it_has_since_exited() {
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
        assert_eq!(gateway_start_poll(false, false, false), None);
        assert_eq!(
            gateway_start_poll(false, false, true),
            Some(GatewayStart::TimedOut)
        );
    }

    #[test]
    fn a_fast_start_says_exactly_what_it_always_said() {
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
        let status = StartupStatus::default();
        status.note_failure("Could not start the background service: nope.");
        status.enter(StartupPhase::EnsuringService);
        status.clear_failure();
        status.enter(StartupPhase::WaitingForGateway);
        aged(&status, Duration::from_secs(20));

        assert_eq!(status.label(), "Waiting for the background service… (20s)");
    }

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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SERVICE_ERR_LOG);
        let mut bytes = format!("{SERVICE_BOOT_MARKER} (pid 1)\n").into_bytes();
        bytes.extend_from_slice(&[0xff, 0xfe]);
        bytes.extend_from_slice(b"\n[service] boot failed: no engine binary\n");
        std::fs::write(&path, &bytes).unwrap();

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
    }

    #[test]
    fn an_absent_log_reads_as_no_evidence() {
        let tail = ServiceLogTail::start(Path::new("/no/such/engine-service.err.log"));
        assert_eq!(tail.from, 0);
        assert_eq!(tail.read(), ServiceBootReport::default());
    }

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
        assert!(plist.contains("Application Support/com.lucidos.app/logs/engine-service.out.log"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn login_launch_script_opens_the_bundle_through_launch_services() {
        let script = login_launch_script(Path::new("/Applications/Lucidos.app")).unwrap();

        assert!(script.contains("/usr/bin/open -g -a '/Applications/Lucidos.app'"));
        assert!(!script.contains("Contents/MacOS"));
        assert!(script.contains(&format!("--args {LOGIN_FLAG}")));
        assert!(script.contains(" -g "));
        assert!(script.contains(&format!("[ $i -lt {LOGIN_OPEN_ATTEMPTS} ]")));
        assert!(script.contains(&format!("sleep {LOGIN_OPEN_RETRY_SECONDS}")));
        assert!(script.trim_end().ends_with("exit 1"));
        assert!(script.contains("&& exit 0"));
        assert!(script.contains("gave up opening"));
        assert!(script.contains(">&2"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn login_launch_script_quotes_a_bundle_path_with_a_quote_in_it() {
        let script =
            login_launch_script(Path::new("/Users/me/don't-quote-me/Lucidos.app")).unwrap();
        assert!(script.contains(r"-a '/Users/me/don'\''t-quote-me/Lucidos.app'"));
        assert_eq!(sh_parses(&script), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_login_script_parses_as_a_shell_script() {
        let script = login_launch_script(Path::new("/Applications/Lucidos.app")).unwrap();
        assert_eq!(sh_parses(&script), Ok(()));
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
        assert!(!plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("&amp;&amp; exit 0"));
        assert!(!plist.contains("&& exit 0"));
        assert!(plist.contains("Application Support/com.lucidos.app/logs/client-login.out.log"));
        assert!(plist.contains("Application Support/com.lucidos.app/logs/client-login.err.log"));
    }

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

    #[cfg(target_os = "macos")]
    fn plutil_lint(plist: &str) -> Result<(), String> {
        check_with("plutil", &["-lint", "-"], plist)
    }

    #[cfg(target_os = "macos")]
    fn sh_parses(script: &str) -> Result<(), String> {
        check_with("/bin/sh", &["-n"], script)
    }

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

        let awkward =
            desired_login_plist(Path::new("/Users/me/App & <Co>/Lucidos.app"), app_data).unwrap();
        assert_eq!(plutil_lint(&awkward), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn no_login_agent_is_installed_for_an_unbundled_binary() {
        let app_data = Path::new("/Users/me/Library/Application Support/com.lucidos.app");
        assert_eq!(
            desired_login_plist_for_exe(Path::new("/usr/local/bin/lucidos-app"), app_data),
            Ok(None)
        );
        let exe = Path::new("/Applications/Lucidos.app/Contents/MacOS/lucidos-app");
        let plist = desired_login_plist_for_exe(exe, app_data)
            .unwrap()
            .expect("a bundled exe installs a login agent");
        assert!(plist.contains("-a '/Applications/Lucidos.app'"));
    }

    #[test]
    fn a_relaunch_never_carries_the_login_flag_forward() {
        let os = |s: &str| std::ffi::OsString::from(s);

        assert_eq!(
            strip_login_flag([os(LOGIN_FLAG)]),
            Vec::<std::ffi::OsString>::new()
        );
        assert_eq!(
            strip_login_flag([os("--first"), os(LOGIN_FLAG), os("--last")]),
            vec![os("--first"), os("--last")]
        );
        assert_eq!(
            strip_login_flag([os("--login-shell"), os("login")]),
            vec![os("--login-shell"), os("login")]
        );
        assert_eq!(strip_login_flag([]), Vec::<std::ffi::OsString>::new());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_two_agents_are_separate_jobs() {
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
            bundle.system_widgets,
            resources.join(SYSTEM_WIDGETS_RESOURCE_NAME)
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
        let dir = tempfile::tempdir().unwrap();
        stop_embedded_postgres(Path::new("/nonexistent/resources"), dir.path());
    }

    #[test]
    fn resolve_engine_port_defaults_then_persists_and_reads_back() {
        std::env::remove_var("LUCIDOS_ENGINE_PORT");

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        assert_eq!(resolve_engine_port(dir), DEFAULT_ENGINE_PORT);
        let cfg = dir.join("config/engine-port");
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap().trim(),
            DEFAULT_ENGINE_PORT.to_string()
        );

        std::fs::write(&cfg, "6123").unwrap();
        assert_eq!(resolve_engine_port(dir), 6123);

        std::fs::write(&cfg, "nope").unwrap();
        assert_eq!(resolve_engine_port(dir), DEFAULT_ENGINE_PORT);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn support_data_paths_full_wipe_targets_the_bundled_install_only() {
        let home = Path::new("/fake/home");
        let app_data = Path::new("/fake/home/Library/Application Support/com.lucidos.app");
        let paths = support_data_paths(home, app_data, true);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/fake/home/Library/Application Support/com.lucidos.app"),
                PathBuf::from("/fake/home/Library/WebKit/com.lucidos.app"),
                PathBuf::from("/fake/home/Library/WebKit/lucidos-app"),
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
        assert!(!paths.contains(&app_data.to_path_buf()));
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
        let elsewhere = Path::new("/Users/someone/Desktop/Lucidos.app/Contents/MacOS/Lucidos");
        assert_eq!(
            app_bundle_root_from_exe(elsewhere),
            Some(PathBuf::from("/Users/someone/Desktop/Lucidos.app"))
        );
        assert_eq!(
            app_bundle_root_from_exe(Path::new("/usr/local/bin/lucidos")),
            None
        );
        assert_eq!(app_bundle_root_from_exe(Path::new("/lucidos")), None);
    }

    #[cfg(target_os = "macos")]
    const INSTALLED_BUNDLE: &str = "/Applications/Lucidos.app";
    #[cfg(target_os = "macos")]
    const INSTALLED_EXE: &str = "/Applications/Lucidos.app/Contents/MacOS/lucidos-app";

    #[cfg(target_os = "macos")]
    fn watcher_script(pid: u32) -> String {
        relaunch_watcher_script(
            pid,
            Path::new(INSTALLED_BUNDLE),
            &[PathBuf::from(INSTALLED_EXE)],
            &[],
        )
        .expect("an ASCII path and no args build a script")
    }

    #[cfg(target_os = "macos")]
    fn end_of_wait(script: &str) -> usize {
        script.find("done;").expect("the wait loop ends")
    }

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

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_stops_waiting_eventually() {
        let script = watcher_script(4242);
        assert!(
            script.contains(&format!("[ $i -lt {RELAUNCH_WAIT_PROBES} ]")),
            "the wait must be bounded, got: {script}"
        );
        assert!(
            script.contains(&format!("[ $a -lt {RELAUNCH_OPEN_ATTEMPTS} ]")),
            "the asking must be bounded too, got: {script}"
        );
        let launch = script.find("/usr/bin/open").expect("it launches the app");
        assert!(
            end_of_wait(&script) < launch,
            "the launch follows the wait loop: {script}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_launches_only_once_the_client_is_actually_gone() {
        let script = watcher_script(4242);
        let done = end_of_wait(&script);
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
            script[guard..launch].contains("exit 1"),
            "a client still alive must leave WITHOUT launching: {script}",
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_keeps_asking_until_the_client_is_actually_back() {
        let script = watcher_script(4242);
        let launch = script.find("/usr/bin/open").expect("it launches the app");
        let confirm = script
            .find("/usr/bin/pgrep")
            .expect("it confirms the launch");
        assert!(launch < confirm, "ask, then confirm: {script}");
        assert!(
            script[confirm..].contains("&& {")
                && script[confirm..].contains("--record-relaunch-client-observed; exit 0;"),
            "a confirmed client is the only way out of the loop: {script}",
        );
        assert!(
            script.contains(&format!("sleep {RELAUNCH_OPEN_RETRY_SECONDS}")),
            "the confirmation gives the client time to appear: {script}",
        );
    }

    #[cfg(target_os = "macos")]
    fn ere_matches(pattern: &str, cmdline: &str) -> bool {
        check_with("/usr/bin/grep", &["-qE", pattern], cmdline).is_ok()
    }

    #[cfg(target_os = "macos")]
    fn probe(args: &[&str]) -> String {
        running_pattern(&[PathBuf::from(INSTALLED_EXE)], args).expect("an ASCII path builds")
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_probe_tells_the_client_apart_from_the_always_running_service() {
        let pattern = probe(&[]);
        assert!(
            ere_matches(&pattern, INSTALLED_EXE),
            "the relaunched client must match: {pattern}"
        );
        assert!(
            !ere_matches(&pattern, &format!("{INSTALLED_EXE} --service")),
            "the service must NOT satisfy the confirmation: {pattern}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_probe_matches_neither_the_watcher_nor_the_open_it_runs() {
        let pattern = probe(&[]);
        for other in [
            "/bin/sh -c i=0; while kill -0 1; do sleep 0.1; done; /usr/bin/open -a '/Applications/Lucidos.app'",
            "/usr/bin/open -a /Applications/Lucidos.app",
            "/Applications/Lucidos.app/Contents/MacOS/lucidos-app-helper",
        ] {
            assert!(!ere_matches(&pattern, other), "{pattern} matched {other}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_probe_expects_the_argv_the_relaunch_is_about_to_ask_for() {
        let pattern = probe(&["--kiosk"]);
        assert!(ere_matches(&pattern, &format!("{INSTALLED_EXE} --kiosk")));
        assert!(
            !ere_matches(&pattern, INSTALLED_EXE),
            "an argv-less client is not the one we asked for: {pattern}"
        );
        assert!(
            !ere_matches(&pattern, &format!("{INSTALLED_EXE} --kiosk --service")),
            "the closing anchor must still hold: {pattern}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_ere_harness_can_fail() {
        assert!(ere_matches("^a$", "a"));
        assert!(!ere_matches("^a$", "b"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_probe_escapes_a_bundle_path_that_looks_like_a_regex() {
        assert_eq!(
            ere_escape("/Users/me/C++ (old)/Lucidos.app"),
            r"/Users/me/C\+\+ \(old\)/Lucidos\.app"
        );
        assert_eq!(ere_escape("plain"), "plain");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_probe_accepts_every_form_the_relaunched_client_might_report() {
        let pattern = running_pattern(
            &[PathBuf::from("/tmp/a"), PathBuf::from("/private/tmp/a")],
            &[],
        )
        .expect("two forms build");
        assert_eq!(pattern, "^(/tmp/a|/private/tmp/a)$");
        assert!(ere_matches(&pattern, "/tmp/a"));
        assert!(ere_matches(&pattern, "/private/tmp/a"));
        assert!(!ere_matches(&pattern, "/private/tmp/a --service"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_probe_with_no_executable_to_look_for_is_refused() {
        assert!(running_pattern(&[], &[]).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_path_with_nothing_to_resolve_contributes_one_form() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("lucidos-app");
        std::fs::write(&exe, b"#!/bin/sh\n").expect("write a stand-in executable");
        let forms = relaunched_exe_forms(&exe);
        assert!(forms.contains(&exe), "the path we were given is always in");
        assert!(
            forms.len() <= 2,
            "one form, or one plus the resolved: {forms:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_unresolvable_path_still_contributes_itself() {
        let exe = PathBuf::from("/Applications/Gone.app/Contents/MacOS/gone");
        assert_eq!(relaunched_exe_forms(&exe), vec![exe]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_never_asks_for_a_second_instance() {
        let script = watcher_script(4242);
        assert!(!script.contains("open -n"), "got: {script}");
        assert!(!script.contains(" -n "), "got: {script}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_way_out_without_a_client_says_so() {
        let script = watcher_script(4242);
        assert!(
            script.contains("never exited"),
            "the client that would not die must be named: {script}"
        );
        assert!(
            script.contains("gave up on"),
            "the exhausted retry must be named: {script}"
        );
        assert_eq!(
            script.matches(">&2").count(),
            3,
            "both dead ends and the refused open report: {script}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_quotes_an_awkward_bundle_path() {
        let bundle = Path::new("/Users/me/My Apps/It's Lucidos.app");
        let exe = bundle.join("Contents/MacOS/lucidos-app");
        let script =
            relaunch_watcher_script(1, bundle, &[exe], &[]).expect("an awkward path still builds");
        assert!(
            script.contains(r#"open -a '/Users/me/My Apps/It'\''s Lucidos.app'"#),
            "got: {script}"
        );
        assert_eq!(sh_parses(&script), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_neutralises_a_bundle_path_the_shell_would_expand() {
        let bundle = Path::new("/Users/me/$HOME `whoami`/Lucidos.app");
        let exe = bundle.join("Contents/MacOS/lucidos-app");
        let script = relaunch_watcher_script(1, bundle, &[exe], &[])
            .expect("an expandable path still builds");
        assert_eq!(sh_parses(&script), Ok(()));
        assert!(
            !script.contains("\"[relaunch]"),
            "no message may sit inside double quotes: {script}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_script_parses_as_a_shell_script() {
        assert_eq!(sh_parses(&watcher_script(4242)), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_reports_only_fixed_lifecycle_outcomes() {
        let script = watcher_script(4242);
        let launch = script.find("/usr/bin/open").unwrap();
        let confirm = script.find("/usr/bin/pgrep").unwrap();
        assert!(script[..launch].contains("--record-relaunch-client-still-running; exit 1;"));
        assert!(script[confirm..].contains("--record-relaunch-client-observed; exit 0;"));
        assert!(script.ends_with("--record-relaunch-attempts-exhausted; exit 1"));
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

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_forwards_arguments_and_omits_the_flag_when_there_are_none() {
        use std::ffi::OsString;

        let args = vec![OsString::from("--flag"), OsString::from("a value")];
        let with = relaunch_watcher_script(
            1,
            Path::new(INSTALLED_BUNDLE),
            &[PathBuf::from(INSTALLED_EXE)],
            &args,
        )
        .expect("args build");
        assert!(
            with.contains(r#"--args '--flag' 'a value' ||"#),
            "the arguments ride on the open, got: {with}"
        );
        assert!(!watcher_script(1).contains("--args"), "no args, no flag");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_watcher_refuses_arguments_it_cannot_quote() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let args = vec![OsString::from_vec(vec![0x2d, 0x2d, 0xff])];
        assert!(
            relaunch_watcher_script(
                1,
                Path::new(INSTALLED_BUNDLE),
                &[PathBuf::from(INSTALLED_EXE)],
                &args
            )
            .is_err(),
            "a non-UTF-8 argument must not be silently mangled into the script",
        );
    }

    struct TempAppData(tempfile::TempDir);

    impl TempAppData {
        fn new() -> Self {
            Self(tempfile::tempdir().expect("create the temp app-data dir"))
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn record(&self) -> Option<String> {
            std::fs::read_to_string(next_boot_path(self.path())).ok()
        }

        fn write_unowned_pidfile(&self, id: &str, pid: u32) {
            let ws = self.path().join("workspaces").join(id);
            std::fs::create_dir_all(ws.join(".lucidos")).expect("create the workspace dir");
            std::fs::write(lucidos_owner_lock::engine::pid_path(&ws), pid.to_string())
                .expect("write the pidfile");
        }

        fn own_as_engine(&self, id: &str, pid: u32) -> lucidos_owner_lock::OwnerLock {
            let ws = self.path().join("workspaces").join(id);
            let lock =
                lucidos_owner_lock::OwnerLock::acquire(&lucidos_owner_lock::engine::lock_path(&ws))
                    .expect("the scratch workspace's engine lock is free");
            self.write_unowned_pidfile(id, pid);
            lock
        }

        fn write_ports(&self, id: &str, contents: &str) {
            let dir = self.path().join("workspaces").join(id).join(".lucidos");
            std::fs::create_dir_all(&dir).expect("create the workspace dir");
            std::fs::write(dir.join("ports"), contents).expect("write the ports file");
        }

        fn write_device_ids(&self, json: &str) {
            let dir = self.path().join("config");
            std::fs::create_dir_all(&dir).expect("create the config dir");
            std::fs::write(dir.join("device-ids.json"), json).expect("write the device map");
        }
    }

    #[test]
    fn the_restore_record_is_the_shape_the_gateway_reads() {
        let tmp = TempAppData::new();
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(r#"{"restore":["myws"]}"#));
        assert_eq!(NEXT_BOOT_QUIT, r#"{"quit":true}"#);
        assert_eq!(NEXT_BOOT_FILE, ".next-boot.json");
    }

    #[test]
    fn a_declared_quit_survives_the_teardown_that_follows_it() {
        let tmp = TempAppData::new();
        declare_quit_intent(tmp.path());
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(NEXT_BOOT_QUIT));
    }

    #[test]
    fn a_quit_intent_can_be_taken_back_when_the_stop_fails() {
        let tmp = TempAppData::new();
        declare_quit_intent(tmp.path());
        clear_next_boot_record(tmp.path());
        assert_eq!(tmp.record(), None);
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(r#"{"restore":["myws"]}"#));
    }

    #[test]
    fn a_workspace_named_quit_is_not_a_quit_intent() {
        let tmp = TempAppData::new();
        record_workspaces_to_restore(tmp.path(), &["quit".to_string()]);
        assert!(!quit_was_declared(&next_boot_path(tmp.path())));
        record_workspaces_to_restore(tmp.path(), &["myws".to_string()]);
        assert_eq!(tmp.record().as_deref(), Some(r#"{"restore":["myws"]}"#));
    }

    #[test]
    fn clearing_a_record_that_is_not_there_is_fine() {
        let tmp = TempAppData::new();
        clear_next_boot_record(tmp.path());
        assert_eq!(tmp.record(), None);
    }

    #[test]
    fn stopping_nothing_clears_any_stale_record() {
        let tmp = TempAppData::new();
        record_workspaces_to_restore(tmp.path(), &["stale".to_string()]);
        record_workspaces_to_restore(tmp.path(), &[]);
        assert_eq!(tmp.record(), None);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_engines_that_were_alive_are_recorded() {
        let tmp = TempAppData::new();
        let mut child = spawn_stand_in_engine();
        let _owner = tmp.own_as_engine("live-ws", child.id());
        tmp.write_unowned_pidfile("stale-ws", 999_999);

        let stopped = stop_workspace_engines(tmp.path());
        assert_eq!(
            stopped,
            vec![SignalledEngine {
                id: "live-ws".to_string(),
                pid: child.id() as i32,
            }],
            "the pid comes back too, so a caller can wait the engine out",
        );
        assert!(
            lucidos_owner_lock::engine::pid_path(&tmp.path().join("workspaces/live-ws")).exists(),
            "the pidfile is the engine's to write, so a stop leaves it",
        );
        let _ = child.wait();
    }

    #[cfg(unix)]
    fn spawn_stand_in_engine() -> std::process::Child {
        Command::new(stand_in_engine_exe())
            .args([
                "--ignored",
                "--exact",
                "desktop::tests::stand_in_engine_body",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a stand-in engine")
    }

    #[test]
    #[ignore = "spawned by the stop tests as a stand-in engine"]
    fn stand_in_engine_body() {
        std::thread::sleep(Duration::from_secs(60));
    }

    #[cfg(unix)]
    fn stand_in_engine_exe() -> &'static Path {
        static EXE: std::sync::OnceLock<(tempfile::TempDir, PathBuf)> = std::sync::OnceLock::new();
        &EXE.get_or_init(|| {
            let dir = tempfile::tempdir().expect("create the stand-in dir");
            let exe = dir.path().join(ENGINE_RESOURCE_NAME);
            std::fs::copy(std::env::current_exe().expect("test binary"), &exe)
                .expect("copy the test binary under the engine's name");
            (dir, exe)
        })
        .1
    }

    #[cfg(unix)]
    #[test]
    fn a_reused_pid_is_not_signalled_as_an_engine() {
        let tmp = TempAppData::new();
        let mut bystander = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn an unrelated process");
        let _owner = tmp.own_as_engine("stopped-ws", bystander.id());

        assert_eq!(stop_workspace_engines(tmp.path()), Vec::new());
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            matches!(bystander.try_wait(), Ok(None)),
            "the unrelated process must still be running",
        );
        let _ = bystander.kill();
        let _ = bystander.wait();
    }

    #[cfg(unix)]
    #[test]
    fn a_waiting_successor_is_stopped_too() {
        let tmp = TempAppData::new();
        let mut successor = spawn_stand_in_engine();
        let ws = tmp.path().join("workspaces/myws");
        std::fs::create_dir_all(ws.join(".lucidos")).unwrap();
        let waiting = lucidos_owner_lock::OwnerLock::acquire(
            &lucidos_owner_lock::engine::successor_lock_path(&ws),
        )
        .unwrap();
        std::fs::write(
            lucidos_owner_lock::engine::successor_pid_path(&ws),
            successor.id().to_string(),
        )
        .unwrap();

        let stopped = stop_workspace_engines(tmp.path());
        drop(waiting);
        let _ = successor.wait();
        assert_eq!(
            stopped,
            vec![SignalledEngine {
                id: "myws".to_string(),
                pid: successor.id() as i32,
            }],
        );
    }

    #[test]
    fn a_pidfile_no_engine_owns_names_no_engine() {
        let tmp = TempAppData::new();
        tmp.write_unowned_pidfile("waiting-ws", 4242);
        let ws = tmp.path().join("workspaces/waiting-ws");
        std::fs::write(lucidos_owner_lock::engine::lock_path(&ws), "").unwrap();
        assert_eq!(read_engine_pid(&ws), None);
        assert_eq!(stop_workspace_engines(tmp.path()), Vec::new());
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
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(delete_path(&missing).is_ok());
    }

    const PACKAGED_PORTS: &str = "API_PORT=49213\nPROTO=http\nPG_PORT=5433\nVITE_PORT=5173\n";

    fn target(port: u16, device_id: &str) -> RestartIntentTarget {
        RestartIntentTarget {
            slug: "myws".to_string(),
            port,
            scheme: "http".to_string(),
            device_id: device_id.to_string(),
        }
    }

    #[test]
    fn a_pidfile_that_would_broadcast_names_no_engine() {
        let tmp = TempAppData::new();
        let ws = tmp.path().join("workspaces/bad-ws");
        let _owner = tmp.own_as_engine("bad-ws", 4242);
        for bad in ["0", "-1", "", "not-a-pid"] {
            std::fs::write(lucidos_owner_lock::engine::pid_path(&ws), bad)
                .expect("write the pidfile");
            assert_eq!(read_engine_pid(&ws), None, "{bad:?}");
        }
        tmp.write_unowned_pidfile("bad-ws", 4242);
        assert_eq!(read_engine_pid(&ws), Some(4242));
    }

    #[test]
    fn the_ports_file_yields_the_engine_port_and_scheme() {
        assert_eq!(
            parse_engine_ports(PACKAGED_PORTS),
            Some((49213, "http".to_string())),
        );
    }

    #[test]
    fn an_absent_proto_reads_as_https() {
        assert_eq!(
            parse_engine_ports("API_PORT=49213\n"),
            Some((49213, "https".to_string())),
        );
    }

    #[test]
    fn a_ports_file_naming_no_usable_port_yields_nothing() {
        for contents in [
            "PROTO=http\n",
            "API_PORT=\n",
            "API_PORT=nope\n",
            "API_PORT=0\n",
        ] {
            assert_eq!(parse_engine_ports(contents), None, "{contents:?}");
        }
    }

    #[test]
    fn a_running_packaged_workspace_is_told_on_its_own_port() {
        assert_eq!(
            restart_intent_target("myws", Some(PACKAGED_PORTS), Some("device-7")),
            Ok(target(49213, "device-7")),
        );
    }

    #[test]
    fn a_workspace_we_cannot_speak_for_is_skipped_for_a_named_reason() {
        assert_eq!(
            restart_intent_target("myws", None, Some("device-7")),
            Err(RestartIntentSkip::NoPort),
        );
        assert_eq!(
            restart_intent_target("myws", Some(PACKAGED_PORTS), None),
            Err(RestartIntentSkip::NoDevice),
        );
    }

    #[test]
    fn an_engine_serving_tls_is_told_over_tls() {
        let tls = "API_PORT=49213\nPROTO=https\n";
        let target = restart_intent_target("myws", Some(tls), Some("device-7")).unwrap();
        assert_eq!(target.scheme, "https");
    }

    #[test]
    fn every_skip_explains_itself() {
        for skip in [RestartIntentSkip::NoPort, RestartIntentSkip::NoDevice] {
            assert!(!skip.reason().is_empty(), "{skip:?}");
        }
    }

    #[test]
    fn a_device_id_that_is_not_header_safe_is_refused() {
        for id in ["", "   ", "dev\r\nX-Evil: 1", "dev ice"] {
            assert_eq!(
                restart_intent_target("myws", Some(PACKAGED_PORTS), Some(id)),
                Err(RestartIntentSkip::NoDevice),
                "{id:?}",
            );
        }
    }

    #[test]
    fn the_request_names_the_device_and_claims_no_proxy_hop() {
        let client = loopback_client().expect("the loopback client");
        let request = restart_intent_request(&client, &target(49213, "device-7"))
            .build()
            .expect("a request");
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "http://127.0.0.1:49213/api/v1/internal/restart-intent"
        );
        assert_eq!(request.headers()[HEADER_DEVICE_ID], "device-7");
        assert!(
            !request.headers().contains_key("x-forwarded-prefix"),
            "a restart intent claiming a proxy hop is refused",
        );
    }

    #[test]
    fn every_target_is_told_and_only_the_acknowledged_ones_count() {
        let targets = [target(1, "a"), target(2, "b")];
        let mut asked = Vec::new();
        let told = announce_to_engines(&targets, Instant::now() + Duration::from_secs(5), |t| {
            asked.push(t.port);
            t.port == 1
        });
        assert_eq!(asked, vec![1, 2]);
        assert_eq!(told, 1);
    }

    #[test]
    fn a_spent_budget_stops_the_sweep() {
        let targets = [target(1, "a"), target(2, "b")];
        let mut asked = 0;
        let told = announce_to_engines(&targets, Instant::now(), |_| {
            asked += 1;
            true
        });
        assert_eq!(asked, 0);
        assert_eq!(told, 0);
    }

    #[cfg(unix)]
    #[test]
    fn only_a_live_engine_this_client_knows_is_a_target() {
        let tmp = TempAppData::new();
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn a stand-in engine");
        tmp.write_device_ids(r#"{"live-ws":"device-7","dead-ws":"device-7"}"#);
        let _owner = tmp.own_as_engine("live-ws", child.id());
        tmp.write_ports("live-ws", PACKAGED_PORTS);
        tmp.write_unowned_pidfile("dead-ws", 999_999);
        tmp.write_ports("dead-ws", PACKAGED_PORTS);

        assert_eq!(
            restart_intent_targets(tmp.path()),
            vec![RestartIntentTarget {
                slug: "live-ws".to_string(),
                port: 49213,
                scheme: "http".to_string(),
                device_id: "device-7".to_string(),
            }],
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_device_store_announces_to_nobody() {
        let tmp = TempAppData::new();
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn a stand-in engine");
        let _owner = tmp.own_as_engine("live-ws", child.id());
        tmp.write_ports("live-ws", PACKAGED_PORTS);

        assert!(restart_intent_targets(tmp.path()).is_empty());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_teardown_poll_ends_only_on_a_sighting_or_the_deadline() {
        assert_eq!(teardown_poll(true, false), Some(TeardownWait::Gone));
        assert_eq!(teardown_poll(true, true), Some(TeardownWait::Gone));
        assert_eq!(teardown_poll(false, true), Some(TeardownWait::StillThere));
        assert_eq!(teardown_poll(false, false), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_wait_that_runs_out_of_time_reports_it_still_there() {
        let mut probes = 0;
        let verdict = wait_until_gone(Duration::ZERO, || {
            probes += 1;
            false
        });
        assert_eq!(verdict, TeardownWait::StillThere);
        assert_eq!(probes, 1, "a spent deadline probes once and gives up");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_wait_ends_as_soon_as_the_probe_sees_it_go() {
        let mut probes = 0;
        let verdict = wait_until_gone(LAUNCHD_TEARDOWN_TIMEOUT, || {
            probes += 1;
            probes >= 3
        });
        assert_eq!(verdict, TeardownWait::Gone);
        assert_eq!(probes, 3, "and not one probe more");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_liveness_probe_calls_only_a_missing_process_gone() {
        assert!(!process_is_gone(std::process::id() as i32));
        assert!(process_is_gone(999_999));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn engines_that_are_already_gone_hold_nothing_up() {
        let engines = vec![SignalledEngine {
            id: "dead-ws".to_string(),
            pid: 999_999,
        }];
        assert!(wait_for_engines_to_exit(&engines, Duration::ZERO).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_engine_that_outlives_the_deadline_is_named() {
        let engines = vec![
            SignalledEngine {
                id: "dead-ws".to_string(),
                pid: 999_999,
            },
            SignalledEngine {
                id: "live-ws".to_string(),
                pid: std::process::id() as i32,
            },
        ];
        let still_alive = wait_for_engines_to_exit(&engines, Duration::ZERO);
        assert_eq!(still_alive, vec![engines[1].clone()]);
    }

    #[cfg(unix)]
    #[test]
    fn the_service_teardown_fits_inside_launchds_exit_timeout() {
        assert!(engine_exit_wait(GATEWAY_STOP_TIMEOUT) >= Duration::from_secs(10));
        for spent in [0, 3, 17].map(Duration::from_secs) {
            assert_eq!(
                spent + engine_exit_wait(spent) + SERVICE_TEARDOWN_ALLOWANCE,
                LAUNCHD_EXIT_TIMEOUT
            );
        }
        assert_eq!(engine_exit_wait(LAUNCHD_EXIT_TIMEOUT), Duration::ZERO);
        assert!(ENGINE_KILL_GRACE < SERVICE_TEARDOWN_ALLOWANCE);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_engine_binary_counts_as_an_engine() {
        assert!(executable_is_engine(Path::new(
            "/Applications/Lucidos.app/Contents/Resources/lucidos-engine"
        )));
        assert!(!executable_is_engine(Path::new(
            "/Applications/Lucidos.app/Contents/Resources/lucidos-gateway"
        )));
        assert!(!executable_is_engine(Path::new(
            "/usr/bin/lucidos-engine-helper"
        )));
        assert!(executable_is_engine(Path::new(
            "/opt/lucidos/lucidos-engine (deleted)"
        )));
        assert!(!pid_runs_engine(std::process::id() as i32));
        assert!(!pid_runs_engine(999_999));
        assert!(pid_runs_another_program(std::process::id() as i32));
        assert!(!pid_runs_another_program(999_999));
    }

    #[test]
    fn the_service_plist_declares_the_exit_timeout_the_teardown_budgets() {
        let plist = desired_service_plist(
            Path::new("/Applications/Lucidos.app/Contents/MacOS/Lucidos"),
            Path::new("/Users/me/Library/Application Support/com.lucidos.app"),
        );
        assert!(plist.contains(&format!(
            "<key>ExitTimeOut</key>\n    <integer>{}</integer>",
            LAUNCHD_EXIT_TIMEOUT.as_secs()
        )));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_stopped_stack_may_be_deleted() {
        assert_eq!(delete_may_proceed(TeardownWait::Gone, &[]), Ok(()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_stack_that_would_not_stop_refuses_the_delete() {
        let ids = vec!["myws".to_string()];

        let service = delete_may_proceed(TeardownWait::StillThere, &[]).unwrap_err();
        assert!(service.contains("the background service"), "{service}");
        assert!(service.contains("Nothing was deleted"), "{service}");

        let engines = delete_may_proceed(TeardownWait::Gone, &ids).unwrap_err();
        assert!(engines.contains("workspace myws"), "{engines}");
        assert!(engines.contains("Nothing was deleted"), "{engines}");

        let both = delete_may_proceed(TeardownWait::StillThere, &ids).unwrap_err();
        assert!(
            both.contains("the background service, workspace myws"),
            "{both}"
        );
    }
}
