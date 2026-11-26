use serde::Serialize;
use std::path::{Path, PathBuf};

pub const BUNDLE_IDENTIFIER: &str = "com.lucidos.app";

pub const BUNDLE_NAME: &str = "Lucidos.app";

pub const SERVICE_AGENT_LABEL: &str = "com.lucidos.engine";

pub const LOGIN_AGENT_LABEL: &str = "com.lucidos.client";

pub const INSTALLER_AGENT_PREFIX: &str = "com.lucidos.gateway.";

pub const INSTALLER_UNIT_PREFIX: &str = "lucidos-gateway-";

pub const SYSTEM_APPLICATIONS_DIR: &str = "/Applications";

pub const DEFAULT_GATEWAY_PORT: u16 = 5252;

pub const DEFAULT_DEV_GATEWAY_PORT: u16 = 5251;

const RESERVED_SLUGS: &[&str] = &["runtime", "current", "gateway", "logs"];

const REPO_MARKER: &str = "scripts/web-dev.sh";

const INSTALL_SH_URL: &str = "https://lucidos.dev/install.sh";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallKind {
    DesktopApp,
    HeadlessInstaller,
    SourceCheckout,
}

impl InstallKind {
    pub fn label(self) -> &'static str {
        match self {
            InstallKind::DesktopApp => "Desktop app",
            InstallKind::HeadlessInstaller => "Installer",
            InstallKind::SourceCheckout => "Source checkout",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentManager {
    Launchd,
    Systemd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaunchAgent {
    pub label: String,
    pub path: PathBuf,
    pub manager: AgentManager,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Install {
    pub kind: InstallKind,
    pub name: String,
    pub root: Option<PathBuf>,
    pub version: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub port: Option<u16>,
    pub agents: Vec<LaunchAgent>,
    pub running_here: bool,
    pub removal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortConflict {
    pub port: u16,
    pub installs: Vec<String>,
}

impl PortConflict {
    pub fn fingerprint(&self) -> String {
        format!("{}:{}", self.port, self.installs.join(","))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Inventory {
    pub installs: Vec<Install>,
    pub conflicts: Vec<PortConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRoots {
    pub home: PathBuf,
    pub application_dirs: Vec<PathBuf>,
    pub installer_prefixes: Vec<PathBuf>,
    pub systemd_user_dir: PathBuf,
    pub dev_gateway_port: u16,
}

impl ScanRoots {
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
        let mut installer_prefixes = vec![lucidos_home::path_in(&home)];
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

    pub fn under(home: &Path) -> Self {
        ScanRoots {
            home: home.to_path_buf(),
            application_dirs: vec![home.join("Applications")],
            installer_prefixes: vec![lucidos_home::path_in(home)],
            systemd_user_dir: home.join(".config/systemd/user"),
            dev_gateway_port: DEFAULT_DEV_GATEWAY_PORT,
        }
    }

    fn default_prefix(&self) -> PathBuf {
        lucidos_home::path_in(&self.home)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunningProcess {
    pub exe: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
}

impl RunningProcess {
    pub fn here(data_dir: Option<PathBuf>) -> Self {
        RunningProcess {
            exe: std::env::current_exe().ok(),
            data_dir,
        }
    }

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

pub fn scan_this_machine(data_dir: Option<PathBuf>) -> Inventory {
    match ScanRoots::for_machine() {
        Some(roots) => scan(&roots, &RunningProcess::here(data_dir)),
        None => Inventory::default(),
    }
}

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

fn installer_instances(roots: &ScanRoots, running: &RunningProcess) -> Vec<Install> {
    let default_prefix = roots.default_prefix();
    roots
        .installer_prefixes
        .iter()
        .flat_map(|prefix| instances_under(roots, prefix, &default_prefix, running))
        .collect()
}

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

fn source_checkout(roots: &ScanRoots, running: &RunningProcess) -> Vec<Install> {
    let data_dir = lucidos_home::default_gateway_data_dir_in(&lucidos_home::path_in(&roots.home));
    if !data_dir.is_dir() {
        return Vec::new();
    }
    let repo = running.exe.as_deref().and_then(repo_root_above);
    let name = repo
        .as_deref()
        .and_then(|r| r.file_name())
        .map(|n| format!("Source checkout \"{}\"", n.to_string_lossy()))
        .unwrap_or_else(|| "Source checkout".to_string());
    vec![Install {
        kind: InstallKind::SourceCheckout,
        name,
        version: repo.as_deref().and_then(read_release_file),
        port: Some(roots.dev_gateway_port),
        agents: Vec::new(),
        running_here: running.belongs_to(repo.as_deref(), Some(&data_dir)),
        removal: format!(
            "Stop it with ./scripts/stop.sh, then delete the checkout and {}.",
            data_dir.display()
        ),
        data_dir: Some(data_dir),
        root: repo,
    }]
}

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

fn systemd_unit(roots: &ScanRoots, unit: &str) -> Option<LaunchAgent> {
    let path = roots.systemd_user_dir.join(unit);
    path.is_file().then(|| LaunchAgent {
        label: unit.to_string(),
        path,
        manager: AgentManager::Systemd,
    })
}

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

fn runtime_dir_version(dir: &Path) -> Option<String> {
    runtime_stem_version(dir.file_name()?.to_str()?)
}

pub fn runtime_stem_version(stem: &str) -> Option<String> {
    let rest = stem.strip_prefix("lucidos-")?;
    let (version, triple) = rest.split_once('-')?;
    let plausible = !version.is_empty()
        && !triple.is_empty()
        && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    plausible.then(|| version.to_string())
}

pub fn bundle_short_version(info_plist: &Path) -> Option<String> {
    let text = std::fs::read_to_string(info_plist).ok()?;
    let after_key = text.split_once("<key>CFBundleShortVersionString</key>")?.1;
    let open = after_key.find("<string>")? + "<string>".len();
    let close = after_key[open..].find("</string>")? + open;
    let value = after_key[open..close].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn engine_port(app_data: &Path) -> u16 {
    read_port(&app_data.join("config/engine-port")).unwrap_or(DEFAULT_GATEWAY_PORT)
}

fn read_port(path: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(path).ok()?;
    raw.trim().parse().ok().filter(|p| *p != 0)
}

fn read_release_file(repo: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(repo.join("RELEASE")).ok()?;
    let value = raw.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub fn repo_root_above(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|a| a.join(REPO_MARKER).exists())
        .map(Path::to_path_buf)
}

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
