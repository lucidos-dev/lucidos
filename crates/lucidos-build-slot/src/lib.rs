use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub const ENV_MAX_BUILDS: &str = "LUCIDOS_MAX_CONCURRENT_BUILDS";

pub const ENV_HELD: &str = "LUCIDOS_BUILD_SLOT_HELD";

pub const ENV_WAIT_SOCKET: &str = "LUCIDOS_BUILD_SLOT_WAIT_SOCKET";

#[cfg(unix)]
pub fn notify_background_wait() -> Option<std::os::unix::net::UnixStream> {
    std::os::unix::net::UnixStream::connect(std::env::var_os(ENV_WAIT_SOCKET)?).ok()
}

pub const ENV_LEASE_NICE: &str = "LUCIDOS_BUILD_SLOT_LEASE_NICE";

pub const ENV_POOL_DIR: &str = "LUCIDOS_BUILD_SLOT_DIR";

pub const ENV_NICE: &str = "LUCIDOS_BUILD_SLOT_NICE";

pub const ENV_CLASS: &str = "LUCIDOS_BUILD_SLOT_CLASS";

pub const ENV_LEGACY_PRIORITY: &str = "LUCIDOS_BUILD_SLOT_PRIORITY";

pub const ENV_CARGO_JOBS: &str = "CARGO_BUILD_JOBS";

const GIB: u64 = 1024 * 1024 * 1024;

const DEFAULT_NICE: i32 = 10;

const MAX_NICE: i32 = 19;

const GIB_PER_BUILD: u64 = 12;

const GIB_HOST_RESERVE: u64 = 8;

const MAX_DERIVED_CAPACITY: u64 = 8;

const UNKNOWN_RAM_CAPACITY: usize = 2;

const POLL_BASE: Duration = Duration::from_millis(250);

const CAPACITY_FILE: &str = "capacity";

const WAITING_FILE: &str = "waiting.lock";

const PRIORITY_FILE: &str = "priority.lock";
const BACKGROUND_FILE: &str = "background.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotClass {
    Background,
    Ordinary,
    Priority,
}

impl SlotClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Priority => "priority",
            Self::Background => "background",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [Self::Ordinary, Self::Priority, Self::Background]
            .into_iter()
            .find(|class| class.as_str() == raw.trim())
    }
}

pub fn resolve_class(raw: Option<&str>) -> SlotClass {
    raw.and_then(SlotClass::parse)
        .unwrap_or(SlotClass::Ordinary)
}

pub fn resolve_class_with_legacy(raw: Option<&str>, legacy: Option<&str>) -> SlotClass {
    if raw.is_none() && legacy.map(str::trim) == Some("1") {
        SlotClass::Priority
    } else {
        resolve_class(raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacitySource {
    EnvVar,
    File,
    HostRam,
    Fallback,
}

impl CapacitySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EnvVar => ENV_MAX_BUILDS,
            Self::File => "capacity file",
            Self::HostRam => "host RAM",
            Self::Fallback => "fallback (host RAM unreadable)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    pub value: usize,
    pub source: CapacitySource,
}

pub fn resolve_capacity(
    env_override: Option<&str>,
    file_contents: Option<&str>,
    total_ram_bytes: Option<u64>,
) -> Capacity {
    if let Some(value) = parse_capacity(env_override) {
        return Capacity {
            value,
            source: CapacitySource::EnvVar,
        };
    }
    if let Some(value) = parse_capacity(file_contents) {
        return Capacity {
            value,
            source: CapacitySource::File,
        };
    }
    match total_ram_bytes {
        Some(bytes) => Capacity {
            value: ((bytes / GIB).saturating_sub(GIB_HOST_RESERVE) / GIB_PER_BUILD)
                .clamp(1, MAX_DERIVED_CAPACITY) as usize,
            source: CapacitySource::HostRam,
        },
        None => Capacity {
            value: UNKNOWN_RAM_CAPACITY,
            source: CapacitySource::Fallback,
        },
    }
}

fn parse_capacity(raw: Option<&str>) -> Option<usize> {
    let n: usize = raw?.trim().parse().ok()?;
    (n >= 1).then_some(n)
}

pub fn host_total_ram_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux_ram_bytes(Path::new("/proc"))
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_ram_bytes(proc: &Path) -> Option<u64> {
    let host = std::fs::read_to_string(proc.join("meminfo"))
        .ok()
        .and_then(|text| parse_meminfo_total(&text));
    host.into_iter().chain(linux_cgroup_limit(proc)).min()
}

#[cfg(any(target_os = "linux", test))]
fn linux_cgroup_limit(proc: &Path) -> Option<u64> {
    let groups = std::fs::read_to_string(proc.join("self/cgroup")).ok()?;
    let member = Path::new(groups.lines().find_map(|line| line.strip_prefix("0::"))?);
    let namespace_path = member.strip_prefix("/").ok()?;
    if namespace_path
        .components()
        .any(|part| part == std::path::Component::ParentDir)
    {
        return None;
    }
    let mounts = std::fs::read_to_string(proc.join("self/mountinfo")).ok()?;
    let mut limit = None;
    for line in mounts.lines() {
        let Some((fields, fs)) = line.split_once(" - ") else {
            continue;
        };
        if fs.split_whitespace().next() != Some("cgroup2") {
            continue;
        }
        let mut paths = fields.split_whitespace().skip(3);
        let Some((root, mount)) = paths.next().zip(paths.next()) else {
            continue;
        };
        let root = mountinfo_path(root);
        let mount = mountinfo_path(mount);
        let relative = member.strip_prefix(root).unwrap_or(namespace_path);
        let leaf = Path::new(&mount).join(relative);
        if !leaf.is_dir() {
            continue;
        }
        for directory in leaf.ancestors().take_while(|path| path.starts_with(&mount)) {
            let bound = std::fs::read_to_string(directory.join("memory.max"))
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok());
            limit = limit.into_iter().chain(bound).min();
        }
    }
    limit
}

#[cfg(any(target_os = "linux", test))]
fn mountinfo_path(path: &str) -> String {
    path.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

#[cfg(any(target_os = "linux", test))]
fn parse_meminfo_total(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

pub fn inherited_slot() -> Option<String> {
    std::env::var(ENV_HELD)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildLimits {
    pub nice: i32,
    pub jobs: Option<usize>,
}

impl BuildLimits {
    pub const NONE: Self = Self {
        nice: 0,
        jobs: None,
    };

    pub fn nested(lease_nice: Option<&str>) -> Self {
        let nice = lease_nice
            .and_then(|raw| raw.parse::<i32>().ok())
            .unwrap_or_default()
            .clamp(0, MAX_NICE);
        Self { nice, ..Self::NONE }
    }
}

pub fn resolve_nice(raw: Option<&str>, class: SlotClass) -> i32 {
    let default = match class {
        SlotClass::Ordinary => DEFAULT_NICE,
        SlotClass::Priority => 0,
        SlotClass::Background => 15,
    };
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return default;
    };
    text.parse::<i32>()
        .map(|n| n.clamp(0, MAX_NICE))
        .unwrap_or(default)
}

pub fn cpu_share(ncpu: usize, holders: usize, capacity: usize) -> usize {
    let guaranteed = (ncpu / capacity.max(1)).max(1);
    let allocated = ncpu / holders.max(1);
    allocated.max(guaranteed)
}

pub fn host_ncpu() -> Option<usize> {
    std::thread::available_parallelism().ok().map(|n| n.get())
}

pub struct BuildSlotPool {
    dir: PathBuf,
    capacity: Capacity,
}

impl BuildSlotPool {
    pub fn open() -> Result<Self, BoxError> {
        let dir = default_pool_dir()?;
        Self::open_at(dir)
    }

    pub fn open_at(dir: PathBuf) -> Result<Self, BoxError> {
        std::fs::create_dir_all(&dir)?;
        let file_contents = std::fs::read_to_string(dir.join(CAPACITY_FILE)).ok();
        let capacity = resolve_capacity(
            std::env::var(ENV_MAX_BUILDS).ok().as_deref(),
            file_contents.as_deref(),
            host_total_ram_bytes(),
        );
        Ok(Self { dir, capacity })
    }

    pub fn with_capacity(dir: PathBuf, value: usize) -> Result<Self, BoxError> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            capacity: Capacity {
                value: value.max(1),
                source: CapacitySource::EnvVar,
            },
        })
    }

    pub fn capacity(&self) -> Capacity {
        self.capacity
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn set_capacity(&self, value: usize) -> Result<(), BoxError> {
        if value < 1 {
            return Err("a build-slot capacity of 0 would freeze every build on this host".into());
        }
        std::fs::write(self.dir.join(CAPACITY_FILE), format!("{value}\n"))?;
        Ok(())
    }

    fn slot_path(&self, index: usize) -> PathBuf {
        self.dir.join(format!("slot-{index}.lock"))
    }

    pub fn try_acquire(&self, label: &str) -> Option<BuildSlotGuard> {
        self.try_acquire_as(label, SlotClass::Ordinary)
    }

    pub fn try_acquire_as(&self, label: &str, class: SlotClass) -> Option<BuildSlotGuard> {
        let _background_turn = if class == SlotClass::Background {
            match self.flag_turn(WAITING_FILE) {
                OrdinaryTurn::Open(held) => held,
                OrdinaryTurn::Yield => return None,
            }
        } else {
            None
        };
        let _turn = match class {
            SlotClass::Priority => None,
            SlotClass::Ordinary | SlotClass::Background => match self.ordinary_turn() {
                OrdinaryTurn::Open(held) => held,
                OrdinaryTurn::Yield => return None,
            },
        };
        (0..self.capacity.value).find_map(|index| self.try_acquire_index(index, label))
    }

    fn try_acquire_index(&self, index: usize, label: &str) -> Option<BuildSlotGuard> {
        let path = self.slot_path(index);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .ok()?;
        fs2::FileExt::try_lock_exclusive(&file).ok()?;
        let mut guard = BuildSlotGuard { index, file };
        guard.write_metadata(label);
        Some(guard)
    }

    pub fn acquire(
        &self,
        label: &str,
        max_wait: Option<Duration>,
        class: SlotClass,
        mut on_wait: impl FnMut(Duration),
    ) -> Option<BuildSlotGuard> {
        if let Some(guard) = self.try_acquire_as(label, class) {
            return Some(guard);
        }
        let _waiting = match class {
            SlotClass::Background => self.mark_background_waiting(),
            SlotClass::Ordinary | SlotClass::Priority => self.mark_waiting(),
        };
        let _priority = match class {
            SlotClass::Priority => self.mark_priority_waiting(),
            SlotClass::Ordinary | SlotClass::Background => None,
        };
        let started = Instant::now();
        loop {
            let waited = started.elapsed();
            if max_wait.is_some_and(|cap| waited >= cap) {
                return None;
            }
            on_wait(waited);
            std::thread::sleep(self.poll_interval());
            if let Some(guard) = self.try_acquire_as(label, class) {
                return Some(guard);
            }
        }
    }

    fn poll_interval(&self) -> Duration {
        let jitter = u64::from(std::process::id() % 100);
        POLL_BASE + Duration::from_millis(jitter)
    }

    pub fn mark_waiting(&self) -> Option<WaitingFlag> {
        let file = self.open_flag_file(WAITING_FILE)?;
        fs2::FileExt::lock_shared(&file).ok()?;
        Some(WaitingFlag { file })
    }

    pub fn anyone_waiting(&self) -> bool {
        self.try_anyone_waiting().unwrap_or(true)
    }

    pub fn try_anyone_waiting(&self) -> Result<bool, BoxError> {
        Ok(self.flag_waiting(WAITING_FILE)? || self.flag_waiting(BACKGROUND_FILE)?)
    }

    fn flag_waiting(&self, name: &str) -> Result<bool, BoxError> {
        let file = self
            .open_flag_file(name)
            .ok_or_else(|| format!("Cannot read build waiters: {name}"))?;
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                fs2::FileExt::unlock(&file)?;
                Ok(false)
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
            Err(error) => Err(error.into()),
        }
    }

    pub fn mark_priority_waiting(&self) -> Option<WaitingFlag> {
        let file = self.open_flag_file(PRIORITY_FILE)?;
        fs2::FileExt::lock_shared(&file).ok()?;
        Some(WaitingFlag { file })
    }

    pub fn mark_background_waiting(&self) -> Option<WaitingFlag> {
        let file = self.open_flag_file(BACKGROUND_FILE)?;
        fs2::FileExt::lock_shared(&file).ok()?;
        Some(WaitingFlag { file })
    }

    pub fn priority_waiting(&self) -> bool {
        matches!(self.ordinary_turn(), OrdinaryTurn::Yield)
    }

    fn ordinary_turn(&self) -> OrdinaryTurn {
        self.flag_turn(PRIORITY_FILE)
    }

    fn flag_turn(&self, name: &str) -> OrdinaryTurn {
        let Some(file) = self.open_flag_file(name) else {
            return OrdinaryTurn::Open(None);
        };
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => OrdinaryTurn::Open(Some(file)),
            Err(_) => OrdinaryTurn::Yield,
        }
    }

    fn open_flag_file(&self, name: &str) -> Option<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(name))
            .ok()
    }

    pub fn status(&self) -> Vec<SlotState> {
        (0..self.capacity.value)
            .map(|index| SlotState {
                index,
                holder: self.probe_holder(index),
            })
            .collect()
    }

    pub fn count_held(states: &[SlotState]) -> usize {
        states.iter().filter(|s| s.holder.is_some()).count()
    }

    pub fn held_slots(&self) -> usize {
        Self::count_held(&self.status())
    }

    pub fn granted_limits(
        &self,
        ncpu: Option<usize>,
        caller_jobs: Option<&str>,
        nice_raw: Option<&str>,
        class: SlotClass,
    ) -> BuildLimits {
        let caller_set = caller_jobs.map(str::trim).is_some_and(|v| !v.is_empty());
        let jobs = if caller_set {
            None
        } else {
            ncpu.map(|n| cpu_share(n, self.held_slots(), self.capacity.value))
        };
        BuildLimits {
            nice: resolve_nice(nice_raw, class),
            jobs,
        }
    }

    fn probe_holder(&self, index: usize) -> Option<SlotHolder> {
        let path = self.slot_path(index);
        let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&path) else {
            return None;
        };
        if fs2::FileExt::try_lock_exclusive(&file).is_ok() {
            let _ = fs2::FileExt::unlock(&file);
            return None;
        }
        let mut raw = String::new();
        let _ = file.read_to_string(&mut raw);
        Some(SlotHolder::parse(&raw))
    }
}

pub struct BuildSlotGuard {
    index: usize,
    file: File,
}

impl BuildSlotGuard {
    pub fn index(&self) -> usize {
        self.index
    }

    fn write_metadata(&mut self, label: &str) {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let body = format!(
            "PID={}\nLABEL={}\nCWD={}\nSTARTED_EPOCH={}\n",
            std::process::id(),
            label.replace('\n', " "),
            cwd,
            started
        );
        let _ = self.file.set_len(0);
        let _ = self.file.seek(SeekFrom::Start(0));
        let _ = self.file.write_all(body.as_bytes());
        let _ = self.file.flush();
    }
}

impl Drop for BuildSlotGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

enum OrdinaryTurn {
    Open(Option<File>),
    Yield,
}

pub struct WaitingFlag {
    file: File,
}

impl Drop for WaitingFlag {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Clone)]
pub struct SlotState {
    pub index: usize,
    pub holder: Option<SlotHolder>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotHolder {
    pub pid: Option<u32>,
    pub label: String,
    pub cwd: String,
    pub started_epoch: Option<u64>,
}

impl SlotHolder {
    pub fn parse(raw: &str) -> Self {
        let mut out = Self::default();
        for line in raw.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "PID" => out.pid = value.trim().parse().ok(),
                "LABEL" => out.label = value.trim().to_string(),
                "CWD" => out.cwd = value.trim().to_string(),
                "STARTED_EPOCH" => out.started_epoch = value.trim().parse().ok(),
                _ => {}
            }
        }
        out
    }

    pub fn held_secs(&self) -> Option<u64> {
        let started = self.started_epoch?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(now.saturating_sub(started))
    }
}

pub fn default_pool_dir() -> Result<PathBuf, BoxError> {
    if let Ok(dir) = std::env::var(ENV_POOL_DIR) {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home = lucidos_home::ensure().map_err(|e| format!("no pool to open: {e}"))?;
    Ok(home.join("build-slots"))
}

#[cfg(test)]
mod tests;
