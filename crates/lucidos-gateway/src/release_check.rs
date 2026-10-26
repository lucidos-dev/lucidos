//! The gateway's release check: one poll per install, announced not installed.
//!
//! ADR 0108. The check lives here because the gateway is per-machine and
//! supervises every workspace. A webview timer polled once per open window, and
//! a headless install polled not at all.
//!
//! Two gates stand in front of the request, and both must pass. The deployment
//! must be installed rather than a source checkout, and the machine-global
//! `~/.lucidos/updates.toml` must have the check enabled. The first is fail
//! closed: it refuses whenever it cannot prove the opposite. The second defaults
//! on, because the check is functional rather than telemetry (ADR 0139).
//!
//! The request carries platform, arch and version. It also carries the caller's
//! IP, as any HTTP request does, and `PRIVACY.md` says so.
//!
//! An *update address* in the same file replaces the origin with a plain
//! `latest.json` URL, for testing a release candidate before it is published.
//! That request carries no query parameters. A refused address fails closed:
//! no request goes out and the snapshot reports why.

use futures::StreamExt;
use lucidos_update_address::UpdateAddress;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Where the check asks. The route is published from the maintainer's workspace
/// and its contract is fixed in ADR 0108.
pub const UPDATE_CHECK_ORIGIN: &str = "https://lucidos.dev/api/update-check";

/// How stale the answer may get before a refresh re-polls, and how often the
/// backstop timer fires for a gateway nobody is looking at.
pub const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Floor between two forced checks, so the Settings button cannot be mashed
/// into a burst of outbound requests.
pub const FORCED_POLL_FLOOR: Duration = Duration::from_secs(60);

/// The product name and the running version, and nothing about the user.
///
/// This adds NO information to the request. The version is already a query
/// parameter, and this repeats it where the origin's zone logs can group by it:
/// the plan denies the query dimension and allows the user agent. An absent user
/// agent invites a bot challenge, which is why the gateway sends one at all.
///
/// Nothing else may join it. ADR 0108 requires aggregate counts with no
/// per-request identity, and rejected an anonymous install id outright.
fn user_agent(version: &str) -> String {
    format!("lucidos-gateway/{version}")
}

/// Cap on the response body. The contract answers one small JSON object, so
/// anything larger is a wrong origin rather than a big answer.
const MAX_BODY_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// The preference file
// ---------------------------------------------------------------------------

/// The machine-global `~/.lucidos/updates.toml`, parsed with safe defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatesToml {
    /// `[release_check] enabled`. Default true.
    pub enabled: bool,
    /// `[release_check] update_address`, exactly as stored. Empty means the
    /// default. Kept raw, so a hand-edited bad value is reported, not dropped.
    pub update_address: String,
}

impl Default for UpdatesToml {
    fn default() -> Self {
        UpdatesToml {
            enabled: true,
            update_address: String::new(),
        }
    }
}

impl UpdatesToml {
    /// The stored address under the shared rule. `Ok(None)` is the default.
    pub fn address(&self) -> Result<Option<UpdateAddress>, String> {
        UpdateAddress::parse(&self.update_address)
    }
}

#[derive(Deserialize, Default)]
struct RawUpdatesToml {
    release_check: Option<RawReleaseCheck>,
}

/// No `deny_unknown_fields`, deliberately. A file written before ADR 0139 still
/// carries `notice_acknowledged`, and it must parse with its `enabled` honoured
/// rather than fall back to the defaults.
#[derive(Deserialize)]
struct RawReleaseCheck {
    #[serde(default = "default_true")]
    enabled: bool,
    /// Any TOML value, so a non-string reaches the address rule and is refused
    /// there, rather than failing the whole file back to the defaults.
    #[serde(default)]
    update_address: Option<toml::Value>,
}

fn raw_address(value: Option<toml::Value>) -> String {
    match value {
        None => String::new(),
        Some(toml::Value::String(s)) => s,
        Some(other) => other.to_string(),
    }
}

fn default_true() -> bool {
    true
}

/// `~/.lucidos/updates.toml`. `None` only when `HOME` is unset.
pub fn updates_toml_path() -> Option<PathBuf> {
    Some(lucidos_home::path()?.join("updates.toml"))
}

/// Read and parse the preference at `path`. Any failure yields the defaults.
pub fn read_updates_toml(path: Option<&Path>) -> UpdatesToml {
    match path.and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(contents) => parse_updates_toml(&contents),
        None => UpdatesToml::default(),
    }
}

/// Pure parse. Malformed input warns and yields the defaults, exactly as
/// `net_config::parse_network_toml` does for the bind file.
pub fn parse_updates_toml(contents: &str) -> UpdatesToml {
    match toml::from_str::<RawUpdatesToml>(contents) {
        Ok(raw) => match raw.release_check {
            Some(rc) => UpdatesToml {
                enabled: rc.enabled,
                update_address: raw_address(rc.update_address),
            },
            None => UpdatesToml::default(),
        },
        Err(e) => {
            crate::log!(
                "[ReleaseCheck] malformed ~/.lucidos/updates.toml ({e}); using defaults (enabled)"
            );
            UpdatesToml::default()
        }
    }
}

/// Render the file with its explanatory comments, since the `toml` serializer
/// drops them.
pub fn render_updates_toml(cfg: &UpdatesToml) -> String {
    let address = if cfg.update_address.is_empty() {
        String::new()
    } else {
        format!(
            "update_address = {}\n",
            toml::Value::String(cfg.update_address.clone())
        )
    };
    format!(
        "# Lucidos update check (ADR 0108, ADR 0139).\n\
         #\n\
         # Once an hour the gateway asks lucidos.dev whether a newer Lucidos is\n\
         # published. The request carries your platform, your architecture and\n\
         # the version you run. It also carries your IP address, as any web\n\
         # request does. Nothing else is sent, and nothing installs itself.\n\
         #\n\
         # enabled         false stops the automatic check. It is on by default,\n\
         #                 and the same switch lives in Settings > System >\n\
         #                 Overview > Maintenance. You can still check by hand\n\
         #                 there while it is off.\n\
         # update_address  a latest.json URL to read instead of the published\n\
         #                 release, to test a release candidate. https, or http\n\
         #                 to localhost only. Absent means the default. The same\n\
         #                 Maintenance section shows it while it is set.\n\
         \n\
         [release_check]\n\
         enabled = {}\n\
         {address}",
        cfg.enabled
    )
}

/// The startup log line for a set address, or `None` when it is unset.
pub fn update_address_log_line(cfg: &UpdatesToml) -> Option<String> {
    match cfg.address() {
        Ok(None) => None,
        Ok(Some(address)) => Some(format!(
            "[ReleaseCheck] update address overridden: updates come from {address}, not the published release"
        )),
        Err(e) => Some(format!(
            "[ReleaseCheck] update address refused, no update check runs until it is fixed: {e}"
        )),
    }
}

/// Write the preference atomically (temp plus rename). Its directory is the
/// Lucidos home, so it is asserted as a storage root first.
pub fn write_updates_toml(path: Option<&Path>, cfg: &UpdatesToml) -> std::io::Result<()> {
    let path =
        path.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))?;
    if let Some(parent) = path.parent() {
        lucidos_home::ensure_storage_root(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, render_updates_toml(cfg))?;
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------
// The fail-closed gate
// ---------------------------------------------------------------------------

/// Is this an installed deployment, so polling is allowed at all?
///
/// Both halves are required. `LUCIDOS_PACKAGED=1` is set by each shipped
/// launcher and by nothing in dev, and the checkout test catches an operator who
/// exported it by hand. An unresolvable executable path is a refusal, because
/// the second half cannot then be answered.
pub fn deployment_is_installed(packaged: bool, exe: Option<&Path>) -> bool {
    match (packaged, exe) {
        (true, Some(p)) => crate::build_id::repo_root_above(p).is_none(),
        _ => false,
    }
}

/// How this install takes an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallShape {
    /// The macOS `.app`, updated in place by the client's Tauri updater.
    DesktopApp,
    /// A headless tarball install, updated by re-running `install.sh`.
    InstallerRerun,
}

impl InstallShape {
    /// The wire value the frontend switches on.
    pub fn as_str(self) -> &'static str {
        match self {
            InstallShape::DesktopApp => "desktop-app",
            InstallShape::InstallerRerun => "installer-rerun",
        }
    }
}

/// Which install shape this executable belongs to, read from its path.
///
/// `packaged` alone cannot tell the two apart, and they take different updates.
/// A bundle puts the gateway under `Lucidos.app/Contents/Resources/`, and the
/// installer puts it under `<prefix>/runtime/<stem>/`. Anything else is
/// unrecognised, and the caller then offers a version with no action.
pub fn install_shape(exe: &Path) -> Option<InstallShape> {
    if exe
        .ancestors()
        .any(|a| a.extension().is_some_and(|e| e == "app"))
    {
        return Some(InstallShape::DesktopApp);
    }
    let runtime_dir = exe.parent()?.parent()?;
    (runtime_dir.file_name()? == "runtime").then_some(InstallShape::InstallerRerun)
}

/// The `install.sh` re-run this instance needs, ready to copy.
///
/// The gateway never runs it. On macOS `launchctl bootout` tears down the job's
/// whole process group, so a spawned installer would kill itself mid-replace
/// (ADR 0108).
///
/// `app_data` is `<prefix>/<slug>`, so both come from the running instance.
/// `--prefix` is omitted when it is already the default.
pub fn installer_command(app_data: &Path, default_prefix: Option<&Path>) -> Option<String> {
    let slug = app_data.file_name()?.to_str()?;
    let prefix = app_data.parent()?;
    let mut cmd = format!(
        "curl -fsSL https://lucidos.dev/install.sh | sh -s -- --name {}",
        shell_quote(slug)
    );
    if default_prefix != Some(prefix) {
        cmd.push_str(&format!(
            " --prefix {}",
            shell_quote(&prefix.to_string_lossy())
        ));
    }
    Some(cmd)
}

/// Quote a value for a shell command line, leaving the ordinary case bare.
fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

// ---------------------------------------------------------------------------
// The request and its answer
// ---------------------------------------------------------------------------

/// The `platform` value for a `std::env::consts::OS`, or `None` when Lucidos
/// publishes nothing for it. An unpublished target polls nothing.
pub fn platform_key(os: &str) -> Option<&'static str> {
    match os {
        "macos" => Some("macos"),
        "linux" => Some("linux"),
        _ => None,
    }
}

/// The `arch` value for a `std::env::consts::ARCH`, or `None` when Lucidos
/// publishes nothing for it.
pub fn arch_key(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" => Some("aarch64"),
        "x86_64" => Some("x86_64"),
        _ => None,
    }
}

/// The full request URL. Exactly three parameters, and no others ever.
pub fn check_url(origin: &str, platform: &str, arch: &str, version: &str) -> String {
    format!("{origin}?platform={platform}&arch={arch}&version={version}")
}

#[derive(Deserialize)]
struct CheckResponse {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

/// What the origin published for this target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedRelease {
    pub version: String,
    /// The release notes, when the origin carries them. Optional in the
    /// contract, and absent means the offer shows no "What's new" link.
    pub notes: Option<String>,
}

/// Read the origin's answer. `Ok(None)` means the origin publishes nothing for
/// this target, which is a valid answer rather than a failure.
///
/// A body that is not JSON is an error, never "up to date". Cloudflare Pages
/// answers an unknown path with landing-page HTML at status 200. That already
/// broke the front door once, and `install.sh` sniffs for it.
pub fn parse_response(body: &str) -> Result<Option<PublishedRelease>, String> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('<') {
        return Err("origin served markup, not JSON (wrong route or a soft 404)".to_string());
    }
    let parsed: CheckResponse =
        serde_json::from_str(trimmed).map_err(|e| format!("unreadable answer: {e}"))?;
    let version = parsed
        .version
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    Ok(version.map(|version| PublishedRelease {
        version,
        notes: parsed.notes.filter(|n| !n.trim().is_empty()),
    }))
}

/// Is `candidate` a later version than `current`?
///
/// Compares dot-separated numeric components, padding the shorter side with
/// zeros. A component that is not a number makes the answer false, so an
/// unreadable version offers nothing rather than offering everything. Mirrors
/// `crates/lucidos-app/src/utils/version.ts`.
pub fn version_is_newer(candidate: &str, current: &str) -> bool {
    let parts = |s: &str| -> Option<Vec<u64>> { s.split('.').map(|p| p.parse().ok()).collect() };
    let (Some(a), Some(b)) = (parts(candidate.trim()), parts(current.trim())) else {
        return false;
    };
    for i in 0..a.len().max(b.len()) {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        if av != bv {
            return av > bv;
        }
    }
    false
}

/// The outbound client. Deliberately NOT `proxy::build_client`, which accepts
/// invalid certificates for the loopback hop.
///
/// It follows no redirect, so the answer comes from the origin we asked. It
/// carries no cookie store, because the `cookies` feature is off. A system proxy
/// is honoured, since a corporate network may require one.
///
/// `version` is the one the request also carries in its query string. The caller
/// passes the same value to both, so the user agent cannot drift from it.
fn build_client(version: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(user_agent(version))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("release-check client builds from static settings")
}

// ---------------------------------------------------------------------------
// The check itself
// ---------------------------------------------------------------------------

/// What the gateway knows about its deployment, resolved once at boot.
pub struct Deployment {
    /// `LUCIDOS_PACKAGED` was truthy.
    pub packaged: bool,
    /// This process's executable, from `current_exe`.
    pub exe: Option<PathBuf>,
    /// `<prefix>/<slug>` for an installer instance, used to compose its command.
    pub app_data: PathBuf,
    /// `~/.lucidos`, so a non-default prefix is spelled out in the command.
    pub default_prefix: Option<PathBuf>,
    /// Where the preference lives. `None` means `HOME` is unset.
    pub config_path: Option<PathBuf>,
}

impl Deployment {
    /// Resolve from the environment at gateway startup.
    pub fn from_env(packaged: bool, exe: Option<PathBuf>, app_data: PathBuf) -> Self {
        Deployment {
            packaged,
            exe,
            app_data,
            default_prefix: lucidos_home::path(),
            config_path: updates_toml_path(),
        }
    }
}

#[derive(Default)]
struct ReleaseState {
    /// When a poll was last attempted, successful or not. Drives the staleness
    /// gate, so a failing origin is not hammered.
    last_attempt: Option<Instant>,
    /// When an answer last arrived.
    checked_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The newest published release, when it is later than ours.
    latest: Option<PublishedRelease>,
    /// Why the last poll failed, cleared by the next success.
    ///
    /// Reported on the wire because a failed check must never read as "you are
    /// up to date". Without it the caller sees the unchanged snapshot and
    /// cannot tell a working check that found nothing from one that never ran.
    last_error: Option<String>,
    /// The raw update address the last attempt used. An answer from another
    /// address is stale: it is not shown, and the next refresh polls again.
    polled_address: String,
}

/// Why a preference write was refused.
#[derive(Debug)]
pub enum ConfigWriteError {
    /// The update address breaks the shared rule. Nothing was written.
    InvalidAddress(String),
    Io(std::io::Error),
}

/// The gateway's one release check. Held on `GatewayInner` and shared by the
/// backstop timer and the control endpoints.
pub struct ReleaseCheck {
    origin: String,
    interval: Duration,
    /// True when the deployment may poll: installed, on a published target.
    supported: bool,
    shape: Option<InstallShape>,
    command: Option<String>,
    target: Option<(&'static str, &'static str)>,
    config_path: Option<PathBuf>,
    current_version: &'static str,
    client: reqwest::Client,
    /// Shared with the task that runs a poll, which outlives its caller.
    state: Arc<Mutex<ReleaseState>>,
    /// Held across a poll so concurrent refreshes coalesce into one request.
    poll_lock: Arc<AsyncMutex<()>>,
}

impl ReleaseCheck {
    /// The production check, against the real origin on the real interval.
    pub fn new(dep: &Deployment) -> Self {
        Self::with_origin(dep, UPDATE_CHECK_ORIGIN, POLL_INTERVAL)
    }

    /// The check pointed at an arbitrary origin, for tests.
    pub fn with_origin(dep: &Deployment, origin: &str, interval: Duration) -> Self {
        let installed = deployment_is_installed(dep.packaged, dep.exe.as_deref());
        let target = platform_key(std::env::consts::OS).zip(arch_key(std::env::consts::ARCH));
        let shape = dep.exe.as_deref().and_then(install_shape);
        let command = match shape {
            Some(InstallShape::InstallerRerun) => {
                installer_command(&dep.app_data, dep.default_prefix.as_deref())
            }
            _ => None,
        };
        // Bound once, so the query parameter and the user agent read the same
        // value by construction rather than by two lookups agreeing.
        let current_version = crate::LUCIDOS_RELEASE;
        ReleaseCheck {
            origin: origin.to_string(),
            interval,
            supported: installed && target.is_some(),
            shape,
            command,
            target,
            config_path: dep.config_path.clone(),
            current_version,
            client: build_client(current_version),
            state: Arc::new(Mutex::new(ReleaseState::default())),
            poll_lock: Arc::new(AsyncMutex::new(())),
        }
    }

    /// The newest published version, when the last answer named one later
    /// than ours.
    pub fn latest_version(&self) -> Option<String> {
        let cfg = self.config();
        let st = self.state.lock().unwrap();
        let current = answer_is_current(&cfg, cfg.address().is_ok(), &st);
        st.latest
            .as_ref()
            .filter(|_| current)
            .map(|release| release.version.clone())
    }

    /// The current preference, re-read from disk on every call.
    ///
    /// Re-read rather than cached so turning the check off in Settings takes
    /// effect on the next tick, with no gateway restart.
    pub fn config(&self) -> UpdatesToml {
        read_updates_toml(self.config_path.as_deref())
    }

    /// Apply a partial change, re-reading the file immediately first.
    ///
    /// The partial shape is ADR 0132's: this file is machine-global and every
    /// gateway on the machine writes it, unlike the paired-device store beside
    /// it. `None` carries the stored value through instead of erroring, so a
    /// body that names no field settles rather than failing. It still rewrites
    /// the file, and it still creates one that was never written.
    ///
    /// An address is checked before anything is written, and stored in its
    /// normalised form. `Some("")` resets it to the default.
    pub fn update_config(
        &self,
        enabled: Option<bool>,
        update_address: Option<&str>,
    ) -> Result<(), ConfigWriteError> {
        let address = update_address
            .map(UpdateAddress::parse)
            .transpose()
            .map_err(ConfigWriteError::InvalidAddress)?;
        let mut cfg = read_updates_toml(self.config_path.as_deref());
        if let Some(v) = enabled {
            cfg.enabled = v;
        }
        if let Some(address) = address {
            cfg.update_address = address.map(|a| a.to_string()).unwrap_or_default();
        }
        write_updates_toml(self.config_path.as_deref(), &cfg).map_err(ConfigWriteError::Io)
    }

    /// Log the update address once at startup, when one is set.
    pub fn log_update_address(&self) {
        if let Some(line) = update_address_log_line(&self.config()) {
            crate::log!("{line}");
        }
    }

    /// May a request go out right now? Both gates, in one place.
    ///
    /// The deployment gate is absolute: a dev build never polls, whatever is
    /// asked of it. The preference covers the AUTOMATIC check only, and `force`
    /// is the Settings button, where the click is itself the request. Being
    /// able to ask by hand is what makes turning the check off safe, and
    /// Settings says so.
    fn may_poll(&self, force: bool) -> bool {
        if !self.supported {
            return false;
        }
        force || self.config().enabled
    }

    /// Has enough time passed since the last attempt? A changed address is
    /// due at once, since the stored answer came from somewhere else.
    fn poll_is_due(&self, force: bool) -> bool {
        let floor = if force {
            FORCED_POLL_FLOOR
        } else {
            self.interval
        };
        let address = self.config().update_address;
        let st = self.state.lock().unwrap();
        if st.polled_address != address {
            return true;
        }
        match st.last_attempt {
            Some(at) => at.elapsed() >= floor,
            None => true,
        }
    }

    /// Poll if the answer is stale, then report what we know.
    ///
    /// Concurrent callers serialize on `poll_lock` and re-test staleness inside
    /// it, so N windows asking at once produce one outbound request.
    pub async fn refresh(&self, force: bool) -> Value {
        if self.may_poll(force) && self.poll_is_due(force) {
            let guard = Arc::clone(&self.poll_lock).lock_owned().await;
            if self.poll_is_due(force) {
                self.poll_once(guard).await;
            }
        }
        self.snapshot()
    }

    /// One request, and what it did to the stored answer.
    ///
    /// A failure logs and leaves the previous answer standing, unless that
    /// answer came from another update address. It still moves
    /// `last_attempt`, so a broken origin is asked once an interval rather than
    /// on every refresh.
    ///
    /// `last_attempt` moves when the request FINISHES, not when it starts. That
    /// is what makes a concurrent caller wait for the answer instead of reading
    /// an empty one: a poll in flight still reads as due, so the caller queues
    /// on `poll_lock` rather than returning past it.
    ///
    /// The request runs in its own task, which holds `guard` until the answer
    /// is stored. A caller can stop listening at any await, such as a window
    /// that hides mid-fetch, and must never cancel a request already sent.
    /// That would leave `last_attempt` unmoved, and the next caller would send
    /// another.
    ///
    /// A refused update address sends nothing. `snapshot` reports the refusal.
    async fn poll_once(&self, guard: OwnedMutexGuard<()>) {
        let Some((platform, arch)) = self.target else {
            return;
        };
        let cfg = self.config();
        let url = match cfg.address() {
            Ok(Some(address)) => address.to_string(),
            Ok(None) => check_url(&self.origin, platform, arch, self.current_version),
            Err(_) => return,
        };
        let client = self.client.clone();
        let state = Arc::clone(&self.state);
        let current_version = self.current_version;
        let poll = tokio::spawn(async move {
            let outcome = fetch(&client, &url).await;
            record(&state, &url, cfg.update_address, current_version, outcome);
            drop(guard);
        });
        // A panic in the task already dropped the guard. The next due refresh
        // asks again, and there is no answer here to wait for.
        let _ = poll.await;
    }

    /// The `release_check` object on `GET /~/api/v1/control/gateway/status`.
    ///
    /// An older gateway omits the whole field, and the frontend reads that as
    /// "no offer", which is the ADR 0105 degradation.
    ///
    /// `update_address` is the address in use, or the refused raw value beside
    /// `update_address_error`. A refused address also reads as `last_error` and
    /// offers nothing, so it can never look like "up to date".
    pub fn snapshot(&self) -> Value {
        let cfg = self.config();
        let st = self.state.lock().unwrap();
        let (update_address, address_error) = match cfg.address() {
            Ok(address) => (address.map(|a| a.to_string()), None),
            Err(e) => (Some(cfg.update_address.clone()), Some(e)),
        };
        let current = answer_is_current(&cfg, address_error.is_none(), &st);
        let latest = st.latest.as_ref().filter(|_| current).map(|release| {
            json!({
                "version": release.version,
                "notes": release.notes,
                "install": self.shape.map(InstallShape::as_str),
                "command": self.command,
            })
        });
        json!({
            "enabled": cfg.enabled,
            "supported": self.supported,
            "current_version": self.current_version,
            "checked_at": st.checked_at.filter(|_| current).map(|t| t.to_rfc3339()),
            "last_error": address_error.clone().or_else(|| st.last_error.clone().filter(|_| current)),
            "latest": latest,
            "update_address": update_address,
            "update_address_error": address_error,
        })
    }
}

/// Fetch and read one answer, with the body size capped.
async fn fetch(client: &reqwest::Client, url: &str) -> Result<Option<PublishedRelease>, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("origin answered {}", resp.status()));
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err("answer exceeds the size the contract allows".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    parse_response(&String::from_utf8_lossy(&body))
}

/// Store what one request to `update_address` came back with.
fn record(
    state: &Mutex<ReleaseState>,
    url: &str,
    update_address: String,
    current_version: &str,
    outcome: Result<Option<PublishedRelease>, String>,
) {
    let mut st = state.lock().unwrap();
    st.last_attempt = Some(Instant::now());
    if st.polled_address != update_address {
        // The stored answer came from the other address. A failure here
        // must not let it pass as this address's offer.
        st.latest = None;
        st.checked_at = None;
        st.polled_address = update_address;
    }
    match outcome {
        Ok(published) => {
            st.checked_at = Some(chrono::Utc::now());
            st.latest = published.filter(|p| version_is_newer(&p.version, current_version));
            st.last_error = None;
        }
        Err(e) => {
            crate::log!("[ReleaseCheck] {url} failed: {e}");
            st.last_error = Some(e);
        }
    }
}

/// Whether the stored answer belongs to the address in use. An answer from
/// another address, or any answer under a refused one, offers nothing.
fn answer_is_current(cfg: &UpdatesToml, address_ok: bool, st: &ReleaseState) -> bool {
    address_ok && st.polled_address == cfg.update_address
}

#[cfg(test)]
#[path = "release_check_tests.rs"]
pub(crate) mod tests;
