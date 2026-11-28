use super::*;

const DISK_MONITOR_INTERVAL: Duration = Duration::from_secs(60);

/// INVARIANT: How far above the soft threshold free disk must recover before the
/// alert re-arms. Without the band, a 60 s tick alerts each time free space
/// jitters across the threshold.
const REARM_MARGIN_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// INVARIANT: Longest wait for the notification to be recorded. On a stalled database the
/// emit gives up, and the next tick retries with a fresh reading. A stale
/// number delivered late is the failure this bound prevents.
const ALERT_EMIT_TIMEOUT: Duration = Duration::from_secs(10);

pub const LARGE_FOOTPRINT_BYTES: u64 = 5 * 1024 * 1024 * 1024;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertState {
    Armed,
    Sent,
}

pub struct DiskMonitor {
    pub(super) bus: Arc<EventBus>,
    pub(super) workspace_root: PathBuf,
    pub(super) free_disk: FreeDiskProbe,
    pub(super) soft_cap_bytes: u64,
    pub(super) hard_cap_bytes: u64,
    pub(super) rearm_margin_bytes: u64,
    pub(super) large_footprint_bytes: u64,
    pub(super) emit_timeout: Duration,
    pub(super) cleanup_wake: Arc<Notify>,
    alert: AlertState,
    level: PressureLevel,
}

impl DiskMonitor {
    pub(super) fn new(
        bus: Arc<EventBus>,
        workspace_root: PathBuf,
        cleanup_wake: Arc<Notify>,
    ) -> Self {
        Self {
            bus,
            free_disk: os_free_disk_probe(workspace_root.clone()),
            workspace_root,
            soft_cap_bytes: FREE_DISK_SOFT_BYTES,
            hard_cap_bytes: FREE_DISK_HARD_BYTES,
            rearm_margin_bytes: REARM_MARGIN_BYTES,
            large_footprint_bytes: LARGE_FOOTPRINT_BYTES,
            emit_timeout: ALERT_EMIT_TIMEOUT,
            cleanup_wake,
            alert: AlertState::Armed,
            level: PressureLevel::Comfortable,
        }
    }

    pub fn spawn(
        bus: Arc<EventBus>,
        workspace_root: PathBuf,
        cleanup_wake: Arc<Notify>,
    ) -> tokio::task::JoinHandle<()> {
        let monitor = Self::new(bus, workspace_root, cleanup_wake);
        tokio::spawn(async move { monitor.run_loop().await })
    }

    async fn run_loop(mut self) {
        log_legacy!(
            "[DiskMonitor] starting (interval={:?}, soft_cap={} bytes, rearm_margin={} bytes)",
            DISK_MONITOR_INTERVAL,
            self.soft_cap_bytes,
            self.rearm_margin_bytes,
        );
        loop {
            self.check_once().await;
            tokio::time::sleep(DISK_MONITOR_INTERVAL).await;
        }
    }

    pub(super) async fn check_once(&mut self) {
        let Some(reading) = (self.free_disk)() else {
            return;
        };
        self.wake_cleanup_on_worsening(reading);
        match self.alert {
            AlertState::Sent => {
                if reading.free_bytes >= self.rearm_level(reading) {
                    self.alert = AlertState::Armed;
                }
                return;
            }
            AlertState::Armed if reading.free_bytes >= self.thresholds(reading).soft_bytes => {
                return
            }
            AlertState::Armed => {}
        }

        let lucidos_bytes = self.lucidos_footprint_bytes().await;
        // INVARIANT: The footprint walk can take seconds. Decide and report on what is
        // true now, not on the reading that started this check.
        let Some(fresh) = (self.free_disk)() else {
            return;
        };
        if fresh.free_bytes >= self.thresholds(fresh).soft_bytes {
            log_legacy!(
                "[DiskMonitor] free disk recovered to {:.1} GB before the alert went out, not sending it",
                fresh.free_bytes as f64 / BYTES_PER_GB,
            );
            return;
        }
        if self
            .emit_disk_low_alert(fresh.free_bytes, lucidos_bytes)
            .await
        {
            self.alert = AlertState::Sent;
        }
    }

    fn wake_cleanup_on_worsening(&mut self, reading: DiskReading) {
        let level =
            DiskPressure::classify(Some(reading), self.soft_cap_bytes, self.hard_cap_bytes).level();
        if level > self.level {
            self.cleanup_wake.notify_one();
        }
        self.level = level;
    }

    fn thresholds(&self, reading: DiskReading) -> DiskThresholds {
        DiskThresholds::for_reading(Some(reading), self.soft_cap_bytes, self.hard_cap_bytes)
    }

    fn rearm_level(&self, reading: DiskReading) -> u64 {
        self.thresholds(reading)
            .soft_bytes
            .saturating_add(self.rearm_margin_bytes)
    }

    async fn lucidos_footprint_bytes(&self) -> u64 {
        let root = self.workspace_root.clone();
        match tokio::task::spawn_blocking(move || worktree_footprint_bytes(&root)).await {
            Ok(bytes) => bytes,
            Err(e) => {
                log_legacy!("[DiskMonitor] footprint walk task failed: {}", e);
                0
            }
        }
    }

    async fn emit_disk_low_alert(&self, free_bytes: u64, lucidos_bytes: u64) -> bool {
        let free_gb = free_bytes as f64 / BYTES_PER_GB;
        let lucidos_gb = lucidos_bytes as f64 / BYTES_PER_GB;
        let event = disk_notification(
            "Low disk space on your machine",
            disk_low_body(free_bytes, lucidos_bytes, self.large_footprint_bytes),
        );
        match tokio::time::timeout(self.emit_timeout, self.bus.emit(event)).await {
            Ok(Ok(_)) => {
                log_legacy!(
                    "[DiskMonitor] free disk below soft threshold ({:.1} GB free, Lucidos {:.1} GB), disk-low NotificationCreated recorded",
                    free_gb,
                    lucidos_gb,
                );
                true
            }
            Ok(Err(e)) => {
                log_legacy!(
                    "[DiskMonitor] free disk is {:.1} GB, but the disk-low notification could not be recorded: {}. Retrying next tick with a fresh reading",
                    free_gb,
                    e,
                );
                false
            }
            Err(_) => {
                log_legacy!(
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
        .map(|entry| walk_directory_size_bytes(&entry.path()))
        .fold(0, u64::saturating_add)
}
