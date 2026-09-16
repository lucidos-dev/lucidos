//! Every Lucidos install this user can see, and whether any two of them fight.
//!
//! # Why this crate exists
//!
//! A machine can carry three install vehicles at once: the macOS `.app`, an
//! `install.sh` headless install, and a source checkout. Each writes its own
//! binaries, its own launch agent, its own data dir and its own gateway port.
//! None of them can see the others, and neither uninstaller knows the other
//! exists.

//! That is a trap rather than a nuisance. An `install.sh` install laid down
//! first keeps port 5252. A DMG installed later cannot bind it, so its gateway
//! dies on every launchd respawn while the new client drives the old engine.
//! The user reads a current client version beside a ten-release-old engine, and
//! nothing explains it.

//! # What a conflict is here, and what it deliberately is not
//!
//! Coexistence is supported and must stay silent. Running a source checkout
//! beside the packaged app is an ordinary developer setup, and the two sit on
//! different ports. [`Inventory::conflicts`] therefore reports **port
//! contention** and nothing else. No marker file, no environment variable and
//! no setting is needed. The ports already say which it is.

//! # The scan reads; it never runs anything and never writes
//!
//! Every version here comes from a path or a plist, never from executing a
//! discovered binary. That is a safety property, not an optimization. This code
//! runs at client startup, and would otherwise execute whatever sits at a
//! well-known path. `no_execute_and_no_write` in the test module pins it by
//! scanning this file.

use serde::Serialize;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// The layout, as constants
// ---------------------------------------------------------------------------
//
// `scripts/lib/service.sh` mirrors these for its uninstall report, which names
// the same bundle and the same two agents. `scripts/lib/service_test.sh` greps
// these literals out of this file and fails when the two drift.

/// The `.app` bundle's identifier, and the directory name of its support data.
/// Matches `BUNDLE_IDENTIFIER` in `crates/lucidos-app/src/desktop.rs`.
pub const BUNDLE_IDENTIFIER: &str = "com.lucidos.app";

/// The bundle's on-disk name, under `/Applications` or `~/Applications`.
pub const BUNDLE_NAME: &str = "Lucidos.app";

/// The packaged always-on service agent. Historical label, see the glossary.
pub const SERVICE_AGENT_LABEL: &str = "com.lucidos.engine";

/// The packaged login agent, which brings the client back at login.
pub const LOGIN_AGENT_LABEL: &str = "com.lucidos.client";

/// Prefix of a headless instance's launchd label, completed by its slug.
/// Matches `service_launchd_label` in `scripts/lib/service.sh`.
pub const INSTALLER_AGENT_PREFIX: &str = "com.lucidos.gateway.";

/// Prefix of a headless instance's systemd unit, completed by its slug.
pub const INSTALLER_UNIT_PREFIX: &str = "lucidos-gateway-";

/// Where the system-wide bundle lives when it is not in `~/Applications`.
pub const SYSTEM_APPLICATIONS_DIR: &str = "/Applications";

/// The packaged gateway's port when nothing has overridden it.
///
/// The *stable gateway port*. Paired devices and the Tauri capability URL
/// pattern key on it, so the packaged client never steps off it.
pub const DEFAULT_GATEWAY_PORT: u16 = 5252;

/// The dev gateway's port when nothing has overridden it, one below the
/// packaged default so the two coexist out of the box.
pub const DEFAULT_DEV_GATEWAY_PORT: u16 = 5251;

/// The installer prefix's subdirectories that are never an instance slug.
/// Mirrors the reserved set in `service_is_instance_name`.
const RESERVED_SLUGS: &[&str] = &["runtime", "current", "gateway", "logs"];

/// Marker that identifies a source checkout, as the engine and gateway both
/// resolve it from the running binary rather than from a compile-time path.
const REPO_MARKER: &str = "scripts/web-dev.sh";

/// The published front door, which `--uninstall` delegates through.
const INSTALL_SH_URL: &str = "https://lucidos.dev/install.sh";

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// Which vehicle laid an install down. The three differ in every path they
/// write, and in what removes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallKind {
    /// The macOS `.app`, from the DMG.
    DesktopApp,
    /// One `install.sh` instance, keyed by its slug.
    HeadlessInstaller,
    /// A git checkout launched by `scripts/web-dev.sh`.
    SourceCheckout,
}

impl InstallKind {
    /// How to name this kind to a reader. Settings renders it as-is.
    pub fn label(self) -> &'static str {
        match self {
            InstallKind::DesktopApp => "Desktop app",
            InstallKind::HeadlessInstaller => "Installer",
            InstallKind::SourceCheckout => "Source checkout",
        }
    }
}

/// Which service manager holds a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentManager {
    Launchd,
    Systemd,
}

/// One registered job that can start an install with nobody clicking anything.
/// This is the half that makes a forgotten install keep coming back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaunchAgent {
    /// The launchd label or the systemd unit name.
    pub label: String,
    /// The plist or unit file on disk.
    pub path: PathBuf,
    pub manager: AgentManager,
}

/// One install found on this machine.
///
/// Every field is what the filesystem says, with `None` where it says nothing.
/// A guess would be worse than a gap: the point is that the user can trust it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Install {
    pub kind: InstallKind,
    /// What to call it: the bundle and where it sits, the instance slug, or the
    /// checkout's directory name.
    pub name: String,
    /// Where its binaries live, when that is recorded. A source checkout's root
    /// is known only when this process runs from it.
    pub root: Option<PathBuf>,
    /// Its version, read from a path or a plist. Never from running it.
    pub version: Option<String>,
    /// Its gateway data: registry, embedded Postgres, logs.
    pub data_dir: Option<PathBuf>,
    /// The gateway port it is configured for.
    pub port: Option<u16>,
    pub agents: Vec<LaunchAgent>,
    /// The scanning process belongs to this install.
    pub running_here: bool,
    /// Exactly what removes it, ready to read out or copy.
    pub removal: String,
}

/// Two or more installs configured for one port. Only one can bind it, and the
/// winner is whichever started first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortConflict {
    pub port: u16,
    /// The contending installs, by [`Install::name`], in inventory order.
    pub installs: Vec<String>,
}

impl PortConflict {
    /// A stable digest of THIS conflict, for a surface that must speak once
    /// per state rather than once per launch.
    ///
    /// Scoped to one conflict rather than to the whole machine, because the
    /// surfaces that announce one describe one. A machine-wide digest would
    /// re-raise an acknowledged port-5252 dialog, with its text unchanged,
    /// because something appeared on 5300.
    pub fn fingerprint(&self) -> String {
        format!("{}:{}", self.port, self.installs.join(","))
    }
}

/// What the machine carries, and whether any of it fights.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Inventory {
    pub installs: Vec<Install>,
    pub conflicts: Vec<PortConflict>,
}

impl Inventory {
    /// The install configured for `port`, when exactly one is.
    ///
    /// Ambiguity is deliberately not resolved. Two installs on one port IS the
    /// conflict, and answering with either would be a guess.
    pub fn serving_port(&self, port: u16) -> Option<&Install> {
        let mut matches = self.installs.iter().filter(|i| i.port == Some(port));
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }
}

// ---------------------------------------------------------------------------
// Where to look, and who is asking
// ---------------------------------------------------------------------------

/// The directories a scan reads. Passed in rather than resolved inside the
/// scan, so a fixture tree drives the whole thing with no environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRoots {
    /// The user's home. Every installer and dev path hangs off it.
    pub home: PathBuf,
    /// Where an `.app` bundle may sit, in the order they are searched.
    pub application_dirs: Vec<PathBuf>,
    /// Every `LUCIDOS_PREFIX` to look for instances under. The default is
    /// first, so a command composed against another one says `--prefix`.
    pub installer_prefixes: Vec<PathBuf>,
    /// `${XDG_CONFIG_HOME:-<home>/.config}/systemd/user`.
    pub systemd_user_dir: PathBuf,
    /// What a dev gateway binds, since it records no port of its own.
    pub dev_gateway_port: u16,
}

impl ScanRoots {
    /// The real machine: `$HOME`, both application directories, the honoured
    /// `XDG_CONFIG_HOME`, and both port and prefix overrides.
    ///
    /// `None` when `HOME` is unset, the one case where nothing resolves.
    pub fn for_machine() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let dev_gateway_port = std::env::var("LUCIDOS_DEV_GATEWAY_PORT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|p| *p != 0)
            .unwrap_or(DEFAULT_DEV_GATEWAY_PORT);
        // A custom `LUCIDOS_PREFIX` is visible only to a process carrying it.
        // So this finds an instance the default prefix would miss, and misses
        // one installed under a prefix nobody named. Reporting the prefixes we
        // can see beats reporting none.
        let mut installer_prefixes = vec![home.join(".lucidos")];
        if let Some(extra) = std::env::var_os("LUCIDOS_PREFIX").map(PathBuf::from) {
            if !installer_prefixes.contains(&extra) {
                installer_prefixes.push(extra);
            }
        }
        Some(ScanRoots {
            application_dirs: vec![
                PathBuf::from(SYSTEM_APPLICATIONS_DIR),
                home.join("Applications"),
            ],
            installer_prefixes,
            systemd_user_dir: config.join("systemd/user"),
            dev_gateway_port,
            home,
        })
    }

    /// Everything under one directory, for a fixture tree. Reads no
    /// environment, so the machine running a test cannot perturb it.
    pub fn under(home: &Path) -> Self {
        ScanRoots {
            home: home.to_path_buf(),
            application_dirs: vec![home.join("Applications")],
            installer_prefixes: vec![home.join(".lucidos")],
            systemd_user_dir: home.join(".config/systemd/user"),
            dev_gateway_port: DEFAULT_DEV_GATEWAY_PORT,
        }
    }

    /// The prefix an instance command omits from `--prefix`.
    fn default_prefix(&self) -> PathBuf {
        self.home.join(".lucidos")
    }
}

/// What the scanning process knows about itself, for [`Install::running_here`].
///
/// Both halves earn their place. Several installer instances SHARE one runtime,
/// so the executable alone cannot tell them apart, and the data dir is what
/// does. A caller that knows neither still gets a correct inventory, with
/// nothing marked as running.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunningProcess {
    /// This process's own executable.
    pub exe: Option<PathBuf>,
    /// The gateway data dir this process serves.
    pub data_dir: Option<PathBuf>,
}

impl RunningProcess {
    /// This process, with the data dir it serves when the caller knows it.
    pub fn here(data_dir: Option<PathBuf>) -> Self {
        RunningProcess {
            exe: std::env::current_exe().ok(),
            data_dir,
        }
    }

    /// Does this process belong to an install with these paths?
    ///
    /// The data dir wins when both sides have one, because it is the only thing
    /// that separates two instances off a shared runtime. Otherwise the
    /// executable's location answers.
    fn belongs_to(&self, root: Option<&Path>, data_dir: Option<&Path>) -> bool {
        match (self.data_dir.as_deref(), data_dir) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => match (self.exe.as_deref(), root) {
                (Some(exe), Some(root)) => exe.starts_with(root),
                _ => false,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The scan
// ---------------------------------------------------------------------------

/// Enumerate every install under `roots`.
pub fn scan(roots: &ScanRoots, running: &RunningProcess) -> Inventory {
    let mut installs = Vec::new();
    installs.extend(desktop_apps(roots, running));
    installs.extend(installer_instances(roots, running));
    installs.extend(source_checkout(roots, running));
    let conflicts = port_conflicts(&installs);
    Inventory {
        installs,
        conflicts,
    }
}

/// Scan the real machine. `data_dir` is the gateway data this process serves,
/// when the caller knows it. An empty inventory is honest when `HOME` is unset.
pub fn scan_this_machine(data_dir: Option<PathBuf>) -> Inventory {
    match ScanRoots::for_machine() {
        Some(roots) => scan(&roots, &RunningProcess::here(data_dir)),
        None => Inventory::default(),
    }
}

/// Every `.app` bundle found, in `application_dirs` order.
fn desktop_apps(roots: &ScanRoots, running: &RunningProcess) -> Vec<Install> {
    let data_dir = roots
        .home
        .join("Library/Application Support")
        .join(BUNDLE_IDENTIFIER);
    let mut found = Vec::new();
    for dir in &roots.application_dirs {
        let bundle = dir.join(BUNDLE_NAME);
        if !bundle.is_dir() {
            continue;
        }
        let agents = [SERVICE_AGENT_LABEL, LOGIN_AGENT_LABEL]
            .iter()
            .filter_map(|label| launchd_agent(roots, label))
            .collect();
        let data = data_dir.is_dir().then(|| data_dir.clone());
        found.push(Install {
            kind: InstallKind::DesktopApp,
            name: format!("{BUNDLE_NAME} in {}", dir.display()),
            version: bundle_short_version(&bundle.join("Contents/Info.plist")),
            port: Some(engine_port(&data_dir)),
            running_here: running.belongs_to(Some(&bundle), data.as_deref()),
            root: Some(bundle),
            data_dir: data,
            agents,
            removal: "Open Lucidos and choose Uninstall Lucidos from the Lucidos menu.".to_string(),
        });
    }
    found
}

/// Every registered `install.sh` instance: a subdirectory of the prefix that
/// carries a `port` marker. That marker is the whole of instance discovery, the
/// same rule `service_list_instance_names` applies.
fn installer_instances(roots: &ScanRoots, running: &RunningProcess) -> Vec<Install> {
    let default_prefix = roots.default_prefix();
    roots
        .installer_prefixes
        .iter()
        .flat_map(|prefix| instances_under(roots, prefix, &default_prefix, running))
        .collect()
}

/// The instances registered under one prefix, sorted by slug.
fn instances_under(
    roots: &ScanRoots,
    prefix: &Path,
    default_prefix: &Path,
    running: &RunningProcess,
) -> Vec<Install> {
    let runtime = current_runtime_dir(prefix);
    let version = runtime.as_deref().and_then(runtime_dir_version);
    let Ok(entries) = std::fs::read_dir(prefix) else {
        return Vec::new();
    };
    let mut slugs: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|slug| !RESERVED_SLUGS.contains(&slug.as_str()))
        .filter(|slug| prefix.join(slug).join("port").is_file())
        .collect();
    slugs.sort();
    slugs
        .into_iter()
        .map(|slug| {
            let data_dir = prefix.join(&slug);
            let mut agents: Vec<LaunchAgent> = Vec::new();
            if let Some(a) = launchd_agent(roots, &format!("{INSTALLER_AGENT_PREFIX}{slug}")) {
                agents.push(a);
            }
            let unit = format!("{INSTALLER_UNIT_PREFIX}{slug}.service");
            if let Some(a) = systemd_unit(roots, &unit) {
                agents.push(a);
            }
            let mut name = format!("install.sh instance \"{slug}\"");
            if prefix != default_prefix {
                name.push_str(&format!(" in {}", prefix.display()));
            }
            Install {
                kind: InstallKind::HeadlessInstaller,
                name,
                version: version.clone(),
                port: read_port(&data_dir.join("port")),
                running_here: running.belongs_to(runtime.as_deref(), Some(&data_dir)),
                removal: installer_removal(prefix, &slug, default_prefix),
                root: runtime.clone(),
                data_dir: Some(data_dir),
                agents,
            }
        })
        .collect()
}

/// The dev gateway, when its machine-global data dir exists.
///
/// Its checkout path is recorded nowhere, so `root` and `version` are filled in
/// only when this process runs from a checkout. It still gets a row without
/// them: the port it holds is what a conflict turns on, and dropping the row
/// would hide the very install the user forgot about.
fn source_checkout(roots: &ScanRoots, running: &RunningProcess) -> Vec<Install> {
    let data_dir = roots.home.join(".lucidos/gateway");
    if !data_dir.is_dir() {
        return Vec::new();
    }
    let repo = running.exe.as_deref().and_then(repo_root_above);
    let name = repo
        .as_deref()
        .and_then(|r| r.file_name())
        .map(|n| format!("Source checkout \"{}\"", n.to_string_lossy()))
        .unwrap_or_else(|| "Source checkout (path not recorded)".to_string());
    vec![Install {
        kind: InstallKind::SourceCheckout,
        name,
        version: repo.as_deref().and_then(read_release_file),
        port: Some(roots.dev_gateway_port),
        agents: Vec::new(),
        running_here: running.belongs_to(repo.as_deref(), Some(&data_dir)),
        data_dir: Some(data_dir),
        root: repo,
        removal: "Stop it with ./scripts/stop.sh, then delete the checkout and \
                  ~/.lucidos/gateway."
            .to_string(),
    }]
}

/// Group the installs by configured port, and report every group above one.
///
/// This is the whole discriminator between a deliberate multi-install and the
/// trap. Two installs on two ports produce nothing.
fn port_conflicts(installs: &[Install]) -> Vec<PortConflict> {
    let mut ports: Vec<u16> = installs.iter().filter_map(|i| i.port).collect();
    ports.sort_unstable();
    ports.dedup();
    ports
        .into_iter()
        .filter_map(|port| {
            let names: Vec<String> = installs
                .iter()
                .filter(|i| i.port == Some(port))
                .map(|i| i.name.clone())
                .collect();
            (names.len() > 1).then_some(PortConflict {
                port,
                installs: names,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pure readers
// ---------------------------------------------------------------------------

/// The launchd plist for `label`, when the file exists.
fn launchd_agent(roots: &ScanRoots, label: &str) -> Option<LaunchAgent> {
    let path = roots
        .home
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist"));
    path.is_file().then(|| LaunchAgent {
        label: label.to_string(),
        path,
        manager: AgentManager::Launchd,
    })
}

/// The systemd user unit named `unit`, when the file exists.
fn systemd_unit(roots: &ScanRoots, unit: &str) -> Option<LaunchAgent> {
    let path = roots.systemd_user_dir.join(unit);
    path.is_file().then(|| LaunchAgent {
        label: unit.to_string(),
        path,
        manager: AgentManager::Systemd,
    })
}

/// Where `<prefix>/runtime/current` points, or the sole runtime directory when
/// the link is missing. An ambiguous `runtime/` yields `None` rather than a
/// version picked out of directory order.
pub fn current_runtime_dir(prefix: &Path) -> Option<PathBuf> {
    let runtime = prefix.join("runtime");
    let current = runtime.join("current");
    if current.is_dir() {
        return std::fs::canonicalize(&current).ok().or(Some(current));
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&runtime)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    (dirs.len() == 1).then(|| dirs.remove(0))
}

/// The version in a runtime directory's own name.
fn runtime_dir_version(dir: &Path) -> Option<String> {
    runtime_stem_version(dir.file_name()?.to_str()?)
}

/// The version inside a `lucidos-<version>-<triple>` tarball stem.
///
/// The version carries no dash and the triple carries several, so the first
/// dash after the prefix is the whole of the split. Mirrors
/// `headless_tarball_stem` in `scripts/lib/headless_tarball.sh`.
pub fn runtime_stem_version(stem: &str) -> Option<String> {
    let rest = stem.strip_prefix("lucidos-")?;
    let (version, triple) = rest.split_once('-')?;
    let plausible = !version.is_empty()
        && !triple.is_empty()
        && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    plausible.then(|| version.to_string())
}

/// `CFBundleShortVersionString` out of an XML `Info.plist`.
///
/// A deliberately small reader rather than a plist parser: Apple's tooling and
/// Tauri both emit the XML form, and anything else answers `None`. The value is
/// the `<string>` that follows the key, which is the format's own rule.
pub fn bundle_short_version(info_plist: &Path) -> Option<String> {
    let text = std::fs::read_to_string(info_plist).ok()?;
    let after_key = text.split_once("<key>CFBundleShortVersionString</key>")?.1;
    let open = after_key.find("<string>")? + "<string>".len();
    let close = after_key[open..].find("</string>")? + open;
    let value = after_key[open..close].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// A packaged install's port: its persisted `config/engine-port`, else the
/// stable default. Mirrors `resolve_engine_port` in `desktop.rs`, minus the
/// environment override, which belongs to a process rather than to a disk.
fn engine_port(app_data: &Path) -> u16 {
    read_port(&app_data.join("config/engine-port")).unwrap_or(DEFAULT_GATEWAY_PORT)
}

/// A port written in a one-line marker file.
fn read_port(path: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(path).ok()?;
    raw.trim().parse().ok().filter(|p| *p != 0)
}

/// The umbrella release a checkout is on, from its `RELEASE` file.
fn read_release_file(repo: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(repo.join("RELEASE")).ok()?;
    let value = raw.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The Lucidos checkout above `exe`, or `None` outside one. Mirrors the
/// gateway's `build_id::repo_root_above` and the engine's `paths::repo_root`,
/// which resolve the same marker from the running binary.
pub fn repo_root_above(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|a| a.join(REPO_MARKER).exists())
        .map(Path::to_path_buf)
}

/// The `install.sh` re-run that removes instance `slug`.
///
/// Composed against the published front door, which delegates to the
/// uninstaller, because a user reading this may have no checkout. `--prefix` is
/// spelled out only when it is not the default, exactly as the gateway's own
/// `installer_command` does for the update path.
pub fn installer_removal(prefix: &Path, slug: &str, default_prefix: &Path) -> String {
    let mut cmd = format!(
        "curl -fsSL {INSTALL_SH_URL} | sh -s -- --uninstall --name {}",
        shell_quote(slug)
    );
    if prefix != default_prefix {
        cmd.push_str(&format!(
            " --prefix {}",
            shell_quote(&prefix.to_string_lossy())
        ));
    }
    cmd
}

/// Quote a value for a shell command line, leaving the ordinary case bare.
/// Mirrors the gateway's `release_check::shell_quote`.
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
// Reporting
// ---------------------------------------------------------------------------

/// Plain lines describing every install the caller is NOT removing, for an
/// uninstaller that cannot reach them. Empty when there is nothing left behind.
///
/// An uninstaller that says "done" over a second engine is what turned one
/// tester's afternoon into a day. Each line names the install, its data, and
/// the command that takes it away.
///
/// `removed` is a predicate rather than a kind, because a vehicle is not always
/// removed whole: the app's own uninstall trashes the RUNNING bundle, so a
/// second bundle elsewhere is a leftover like any other.
pub fn leftovers_report(inventory: &Inventory, removed: impl Fn(&Install) -> bool) -> Vec<String> {
    inventory
        .installs
        .iter()
        .filter(|i| !removed(i))
        .flat_map(|i| {
            let version = i
                .version
                .as_deref()
                .map(|v| format!(" version {v}"))
                .unwrap_or_default();
            let mut lines = vec![format!("{} [{}]{version}", i.name, i.kind.label())];
            if let Some(data) = &i.data_dir {
                lines.push(format!("  data: {}", data.display()));
            }
            for agent in &i.agents {
                lines.push(format!("  starts itself: {}", agent.path.display()));
            }
            lines.push(format!("  remove with: {}", i.removal));
            lines
        })
        .collect()
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
