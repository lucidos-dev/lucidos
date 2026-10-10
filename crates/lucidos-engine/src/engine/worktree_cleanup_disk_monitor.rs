//! The disk monitor: watches free space on the workspace volume, sends the
//! "Low disk space on your machine" notification, and wakes the cleanup
//! worker when pressure worsens.
//!
//! It runs apart from the cleanup cycle on purpose. A full disk is what stalls
//! Postgres, and the cleanup cycle waits on Postgres for every worktree. So
//! this task never runs a query: it probes, decides and emits on its own tick.
//! The one database write is the notification itself, bounded by
//! [`ALERT_EMIT_TIMEOUT`]. The reasoning and the degraded behaviour are in
//! `docs/adr/0302-the-disk-monitor-never-waits-on-the-database.md`. The wake
//! is in memory (ADR 0311).

use super::*;

/// How often the monitor reads free disk. A `statvfs` call is cheap, and free
/// space can fall from the soft threshold to zero in minutes.
const DISK_MONITOR_INTERVAL: Duration = Duration::from_secs(60);

/// How far above [`FREE_DISK_SOFT_BYTES`] free disk must recover before the
/// alert re-arms. Without the band, a 60 s tick alerts each time free space
/// jitters across the threshold.
const REARM_MARGIN_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Longest wait for the notification to be recorded. On a stalled database the
/// emit gives up, and the next tick retries with a fresh reading. A stale
/// number delivered late is the failure this bound prevents.
const ALERT_EMIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Threshold above which Lucidos's own worktree footprint is "meaningful"
/// in the disk-low notification. Below this, the heads-up message tells the
/// user the pressure is from their machine overall (other apps), not Lucidos,
/// so the framing matches reality. 5 GB is roughly one CC session's `target/`
/// after a Cargo build. Anything noticeably above "one fresh worktree" gets
/// the cleanup-suggestion variant.
pub const LARGE_FOOTPRINT_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Body for the soft-threshold heads-up.
///
/// The body frames the pressure around the user's machine, not Lucidos: the
/// trigger is system-wide free space, and Lucidos's own footprint is usually a
/// small slice of it. The two branches differ in the REMEDY, never in the
/// destination. A large footprint means cleaning here reclaims real space; a
/// small one means the page is where you confirm the pressure is elsewhere.
pub(super) fn disk_low_body(
    free_bytes: u64,
    lucidos_bytes: u64,
    large_footprint_bytes: u64,
) -> String {
    let free_gb = free_bytes as f64 / BYTES_PER_GB;
    let lucidos_gb = lucidos_bytes as f64 / BYTES_PER_GB;
    let volume = format!("Only {free_gb:.1} GB free on the volume hosting your Lucidos workspace.");
    let page = SettingsPage::DISK_USAGE.link();
    if lucidos_bytes >= large_footprint_bytes {
        format!(
            "{volume} Lucidos worktrees use {lucidos_gb:.1} GB: clean idle ones from \
             {page} to reclaim space. New coding-agent sessions may fail \
             to spawn until disk is freed."
        )
    } else {
        format!(
            "{volume} Lucidos itself uses just {lucidos_gb:.1} GB, so most of the pressure \
             is from other apps on your machine. {page} has the breakdown. \
             New coding-agent sessions may fail to spawn until you free space elsewhere."
        )
    }
}

/// Where the monitor stands in the current pressure episode. It lives in
/// memory only: a restart re-arms it, which costs at most one extra alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertState {
    /// The next reading below the soft threshold sends an alert.
    Armed,
    /// This episode's alert was recorded. Recovery to the re-arm level ends
    /// the episode.
    Sent,
}

/// Task handle returned by [`DiskMonitor::spawn`].
pub struct DiskMonitor {
    pub(super) bus: Arc<EventBus>,
    pub(super) workspace_root: PathBuf,
    pub(super) free_disk: FreeDiskProbe,
    pub(super) free_soft_bytes: u64,
    pub(super) free_hard_bytes: u64,
    pub(super) rearm_margin_bytes: u64,
    pub(super) large_footprint_bytes: u64,
    pub(super) emit_timeout: Duration,
    pub(super) cleanup_wake: Arc<Notify>,
    alert: AlertState,
    /// The level of the last successful reading. Memory only: a restart
    /// starts at `Comfortable`, which costs at most one extra cycle.
    level: PressureLevel,
}

impl DiskMonitor {
    /// Build a monitor with production defaults.
    pub(super) fn new(
        bus: Arc<EventBus>,
        workspace_root: PathBuf,
        cleanup_wake: Arc<Notify>,
    ) -> Self {
        Self {
            bus,
            free_disk: os_free_disk_probe(workspace_root.clone()),
            workspace_root,
            free_soft_bytes: FREE_DISK_SOFT_BYTES,
            free_hard_bytes: FREE_DISK_HARD_BYTES,
            rearm_margin_bytes: REARM_MARGIN_BYTES,
            large_footprint_bytes: LARGE_FOOTPRINT_BYTES,
            emit_timeout: ALERT_EMIT_TIMEOUT,
            cleanup_wake,
            alert: AlertState::Armed,
            level: PressureLevel::Comfortable,
        }
    }

    /// Start the monitor on a tokio task that lives for the engine's lifetime.
    pub fn spawn(
        bus: Arc<EventBus>,
        workspace_root: PathBuf,
        cleanup_wake: Arc<Notify>,
    ) -> tokio::task::JoinHandle<()> {
        let monitor = Self::new(bus, workspace_root, cleanup_wake);
        tokio::spawn(async move { monitor.run_loop().await })
    }

    /// Loop forever. The sleep starts only when a check returns, so checks
    /// never overlap, and a stalled emit holds one check for at most
    /// [`ALERT_EMIT_TIMEOUT`].
    async fn run_loop(mut self) {
        log!(
            "[DiskMonitor] starting (interval={:?}, free_soft={} bytes, rearm_margin={} bytes)",
            DISK_MONITOR_INTERVAL,
            self.free_soft_bytes,
            self.rearm_margin_bytes,
        );
        loop {
            self.check_once().await;
            tokio::time::sleep(DISK_MONITOR_INTERVAL).await;
        }
    }

    /// One reading. A worsening wakes the cleanup worker, and a crossing
    /// below the soft threshold sends one alert. A failed probe changes
    /// nothing, so a transient failure neither wakes, sends nor re-arms.
    pub(super) async fn check_once(&mut self) {
        let Some(free) = (self.free_disk)() else {
            return;
        };
        self.wake_cleanup_on_worsening(free);
        match self.alert {
            AlertState::Sent => {
                if free >= self.rearm_level() {
                    self.alert = AlertState::Armed;
                }
                return;
            }
            AlertState::Armed if free >= self.free_soft_bytes => return,
            AlertState::Armed => {}
        }

        let lucidos_bytes = self.lucidos_footprint_bytes().await;
        // The footprint walk can take seconds. Decide and report on what is
        // true now, not on the reading that started this check.
        let Some(fresh) = (self.free_disk)() else {
            return;
        };
        if fresh >= self.free_soft_bytes {
            log!(
                "[DiskMonitor] free disk recovered to {:.1} GB before the alert went out, not sending it",
                fresh as f64 / BYTES_PER_GB,
            );
            return;
        }
        if self.emit_disk_low_alert(fresh, lucidos_bytes).await {
            self.alert = AlertState::Sent;
        }
    }

    /// Once per worsening, never per tick: a wake per tick would run a full
    /// cleanup cycle every minute through a long low-disk episode.
    fn wake_cleanup_on_worsening(&mut self, free: u64) {
        let level =
            DiskPressure::classify(Some(free), self.free_soft_bytes, self.free_hard_bytes).level();
        if level > self.level {
            self.cleanup_wake.notify_one();
        }
        self.level = level;
    }

    fn rearm_level(&self) -> u64 {
        self.free_soft_bytes.saturating_add(self.rearm_margin_bytes)
    }

    /// Bytes held by `thread-<8hex>` worktrees, measured from disk alone.
    /// Unlike the Disk Usage inventory it counts orphaned directories too,
    /// since telling them apart needs the database.
    async fn lucidos_footprint_bytes(&self) -> u64 {
        let root = self.workspace_root.clone();
        match tokio::task::spawn_blocking(move || worktree_footprint_bytes(&root)).await {
            Ok(bytes) => bytes,
            Err(e) => {
                log!("[DiskMonitor] footprint walk task failed: {}", e);
                0
            }
        }
    }

    /// Record the alert. Returns `false` when the database did not take it
    /// within [`Self::emit_timeout`], which keeps the alert armed for a retry.
    async fn emit_disk_low_alert(&self, free_bytes: u64, lucidos_bytes: u64) -> bool {
        let free_gb = free_bytes as f64 / BYTES_PER_GB;
        let lucidos_gb = lucidos_bytes as f64 / BYTES_PER_GB;
        let event = disk_notification(
            "Low disk space on your machine",
            disk_low_body(free_bytes, lucidos_bytes, self.large_footprint_bytes),
        );
        match tokio::time::timeout(self.emit_timeout, self.bus.emit(event)).await {
            Ok(Ok(_)) => {
                log!(
                    "[DiskMonitor] free disk below soft threshold ({:.1} GB free, Lucidos {:.1} GB), disk-low NotificationCreated recorded",
                    free_gb,
                    lucidos_gb,
                );
                true
            }
            Ok(Err(e)) => {
                log!(
                    "[DiskMonitor] free disk is {:.1} GB, but the disk-low notification could not be recorded: {}. Retrying next tick with a fresh reading",
                    free_gb,
                    e,
                );
                false
            }
            Err(_) => {
                log!(
                    "[DiskMonitor] free disk is {:.1} GB, but recording the disk-low notification took over {:?}. Retrying next tick with a fresh reading",
                    free_gb,
                    self.emit_timeout,
                );
                false
            }
        }
    }
}

fn worktree_footprint_bytes(workspace_root: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(worktrees_dir(workspace_root)) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(parse_thread_short)
                .is_some()
        })
        .map(|entry| directory_size_bytes(&entry.path()))
        .fold(0, u64::saturating_add)
}
