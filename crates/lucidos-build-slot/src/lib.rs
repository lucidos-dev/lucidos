//! *Build slot*: one of N permits to run a heavy build on this host.
//!
//! Every coding-agent worktree has its own `target/`, so N parallel `cargo`
//! builds of `lucidos-engine` are N full compiles resident at once, which OOMs
//! the machine. This crate is the shared pool that stops them piling up. Both
//! the `lucidos build-slot` CLI verb and the engine's own rebuild use it.
//!
//! A slot is an `fs2` flock held by the process running the build, so the
//! kernel releases it on death. A killed build therefore cannot wedge the
//! pool, which is the property the ad-hoc `mkdir` lock this replaces lacked.
//!
//! Deliberately not a queue: whoever samples a freed slot first takes it.
//! Arrival order is an explicit non-goal, because ticket state is exactly the
//! stale state this design removes.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Per-process escape hatch overriding the resolved slot count.
pub const ENV_MAX_BUILDS: &str = "LUCIDOS_MAX_CONCURRENT_BUILDS";

/// Exported into the wrapped child, marking its whole process tree as already
/// holding a slot. A nested acquisition reads it and passes straight through,
/// which is what stops `make test` deadlocking against `test-engine.sh`.
pub const ENV_HELD: &str = "LUCIDOS_BUILD_SLOT_HELD";

/// Test seam: overrides the pool directory that would otherwise sit under
/// `$HOME`. Tests must never touch the real machine-wide pool.
pub const ENV_POOL_DIR: &str = "LUCIDOS_BUILD_SLOT_DIR";

const GIB: u64 = 1024 * 1024 * 1024;

/// Gibibytes of host RAM we assume one heavy build needs.
const GIB_PER_BUILD: u64 = 16;

/// Ceiling on the derived count. Past this, cores rather than memory bind, and
/// nobody wants sixteen concurrent rustc trees on one box.
const MAX_DERIVED_CAPACITY: u64 = 8;

/// Used when host RAM cannot be read. Deliberately not 1: a guard that cannot
/// measure must degrade to cautious, not to serialising every build.
const UNKNOWN_RAM_CAPACITY: usize = 2;

/// Base interval between polls while waiting for a slot.
const POLL_BASE: Duration = Duration::from_millis(250);

/// Name of the machine-wide file holding the configured slot count.
const CAPACITY_FILE: &str = "capacity";

/// File waiters hold a shared lock on. See [`BuildSlotPool::anyone_waiting`].
const WAITING_FILE: &str = "waiting.lock";

/// Where a resolved slot count came from. Reported by status so a host whose
/// participants disagree is visible rather than silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacitySource {
    /// `LUCIDOS_MAX_CONCURRENT_BUILDS` in this process's environment.
    EnvVar,
    /// The machine-wide capacity file next to the pool.
    File,
    /// Derived from host RAM.
    HostRam,
    /// Host RAM could not be read.
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

/// A resolved slot count and the source that supplied it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    pub value: usize,
    pub source: CapacitySource,
}

/// Resolve the slot count from its three sources, in precedence order.
///
/// A value of zero would freeze every build on the host with no way back. So
/// zero and unparseable input are both ignored rather than honoured. That
/// differs from the Thread Queue's *capacity policy*, where 0 means "hold": a
/// held queue is drained by raising the cap, but a frozen build slot would
/// take the machine's own rebuild down with it.
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
            value: ((bytes / GIB) / GIB_PER_BUILD).clamp(1, MAX_DERIVED_CAPACITY) as usize,
            source: CapacitySource::HostRam,
        },
        None => Capacity {
            value: UNKNOWN_RAM_CAPACITY,
            source: CapacitySource::Fallback,
        },
    }
}

/// A positive slot count, or `None` for absent, blank, unparseable or zero.
fn parse_capacity(raw: Option<&str>) -> Option<usize> {
    let n: usize = raw?.trim().parse().ok()?;
    (n >= 1).then_some(n)
}

/// Total host RAM in bytes, or `None` on a platform we cannot read.
pub fn host_total_ram_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_meminfo_total(&text)
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

/// `MemTotal:  16316108 kB` from `/proc/meminfo`, as bytes.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo_total(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// The slot this process holds, when it is already inside a wrapped build.
/// `Some` means a nested acquisition must pass straight through.
pub fn inherited_slot() -> Option<String> {
    std::env::var(ENV_HELD)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The machine-wide pool of build slots.
pub struct BuildSlotPool {
    dir: PathBuf,
    capacity: Capacity,
}

impl BuildSlotPool {
    /// Open the pool for this host, creating its directory if needed.
    ///
    /// Every caller treats an `Err` as fail-open and runs the build
    /// unrestricted: a limiter that cannot open its own pool must never be the
    /// reason a build does not happen.
    pub fn open() -> Result<Self, BoxError> {
        let dir = default_pool_dir()?;
        Self::open_at(dir)
    }

    /// Open a pool rooted at an explicit directory. Used by tests and by the
    /// [`ENV_POOL_DIR`] seam.
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

    /// Open a pool with an explicit count, consulting no environment and no
    /// capacity file. Tests use it so two of them can hold different-sized
    /// pools at once without racing over one process-wide variable.
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

    /// Write the machine-wide capacity file. Rejects zero for the reason
    /// [`resolve_capacity`] documents.
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

    /// Take the first free slot, or `None` when all are held.
    pub fn try_acquire(&self, label: &str) -> Option<BuildSlotGuard> {
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

    /// Block until a slot frees, calling `on_wait` once per poll so the caller
    /// can report progress. Returns `None` only when `max_wait` elapsed.
    ///
    /// The waiting flag is raised BEFORE the first `on_wait`. That is what
    /// lets a caller announce contention from inside the callback and still
    /// be seen by a holder releasing at that moment.
    pub fn acquire(
        &self,
        label: &str,
        max_wait: Option<Duration>,
        mut on_wait: impl FnMut(Duration),
    ) -> Option<BuildSlotGuard> {
        if let Some(guard) = self.try_acquire(label) {
            return Some(guard);
        }
        // Announce that somebody is queued behind the pool, so a releasing
        // holder knows to emit rather than staying silent on the fast path.
        let _waiting = self.mark_waiting();
        let started = Instant::now();
        loop {
            let waited = started.elapsed();
            if max_wait.is_some_and(|cap| waited >= cap) {
                return None;
            }
            on_wait(waited);
            std::thread::sleep(self.poll_interval());
            if let Some(guard) = self.try_acquire(label) {
                return Some(guard);
            }
        }
    }

    /// Poll interval, jittered per process so waiters released together do not
    /// resample in lockstep and thrash the same slot file.
    fn poll_interval(&self) -> Duration {
        let jitter = u64::from(std::process::id() % 100);
        POLL_BASE + Duration::from_millis(jitter)
    }

    /// Take a shared lock on the waiting file for as long as this process is
    /// queued. Reports "somebody is blocked right now" to `--status`.
    ///
    /// Deliberately NOT what gates the release announcement. A waiter that hit
    /// `--max-wait` has exited and taken its flag with it. Gating on this would
    /// go silent for exactly the subscriber that needs waking.
    pub fn mark_waiting(&self) -> Option<WaitingFlag> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(WAITING_FILE))
            .ok()?;
        fs2::FileExt::try_lock_shared(&file).ok()?;
        Some(WaitingFlag { file })
    }

    /// Is somebody blocked on this pool right now?
    ///
    /// Answered by trying to take the waiting file exclusively: success means
    /// no waiter holds it. A probe that cannot run answers `true`, so the
    /// failure direction is a redundant announcement rather than a waiter left
    /// asleep.
    pub fn anyone_waiting(&self) -> bool {
        let Ok(file) = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(WAITING_FILE))
        else {
            return true;
        };
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                let _ = fs2::FileExt::unlock(&file);
                false
            }
            Err(_) => true,
        }
    }

    /// Current occupancy, one entry per slot.
    ///
    /// Probing takes no slot: each file is try-locked and immediately
    /// released, so asking why a build is waiting never makes it wait longer.
    pub fn status(&self) -> Vec<SlotState> {
        (0..self.capacity.value)
            .map(|index| SlotState {
                index,
                holder: self.probe_holder(index),
            })
            .collect()
    }

    fn probe_holder(&self, index: usize) -> Option<SlotHolder> {
        let path = self.slot_path(index);
        // No file yet means the slot has never been taken.
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

/// A held build slot. Dropping it frees the slot, and so does process death.
pub struct BuildSlotGuard {
    index: usize,
    file: File,
}

impl BuildSlotGuard {
    pub fn index(&self) -> usize {
        self.index
    }

    /// Record who holds this slot, for `status` to read back. Best effort: the
    /// slot is already held, and unreadable metadata must not fail a build.
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

/// Evidence that this process is queued for a slot. Held for the wait only.
pub struct WaitingFlag {
    file: File,
}

impl Drop for WaitingFlag {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

/// One slot's occupancy at probe time.
#[derive(Debug, Clone)]
pub struct SlotState {
    pub index: usize,
    pub holder: Option<SlotHolder>,
}

/// Who holds a slot, as recorded by the holder itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotHolder {
    pub pid: Option<u32>,
    pub label: String,
    pub cwd: String,
    pub started_epoch: Option<u64>,
}

impl SlotHolder {
    /// Parse the `KEY=value` metadata block. An unknown key is ignored and a
    /// missing one leaves its field empty. A slot taken by an older build
    /// therefore still reports whatever it did write.
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

    /// Seconds this slot has been held, when the holder recorded a start.
    pub fn held_secs(&self) -> Option<u64> {
        let started = self.started_epoch?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(now.saturating_sub(started))
    }
}

/// `$LUCIDOS_BUILD_SLOT_DIR`, else `$HOME/.lucidos/build-slots`.
///
/// Machine-wide on purpose: host RAM is machine-wide, several workspaces run
/// at once by design, and an external-repo agent session builds outside every
/// Lucidos checkout.
pub fn default_pool_dir() -> Result<PathBuf, BoxError> {
    if let Ok(dir) = std::env::var(ENV_POOL_DIR) {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set, so there is no pool to open")?;
    if home.trim().is_empty() {
        return Err("HOME is empty, so there is no pool to open".into());
    }
    Ok(PathBuf::from(home).join(".lucidos").join("build-slots"))
}

#[cfg(test)]
mod tests;
