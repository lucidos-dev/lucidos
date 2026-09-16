//! The gateway's view of every Lucidos install on this machine.
//!
//! The enumeration itself lives in `lucidos-installs`, shared with the client
//! so the two can never describe the same machine differently. This module is
//! the gateway's two uses of it: the control-plane read that Settings renders,
//! and one summary in the boot log.
//!
//! Nothing is cached. The scan reads a handful of directories, and the route is
//! opened by hand rather than polled. A stale answer here would be the exact
//! failure the feature exists to end.

use lucidos_installs::{Inventory, RunningProcess, ScanRoots};
use std::path::{Path, PathBuf};

/// Every install this user can see, with this gateway marked as the live one.
///
/// `app_data` is what separates two installer instances sharing one runtime:
/// their executables are the same file, and only the data dir differs.
pub fn inventory(exe: Option<PathBuf>, app_data: &Path) -> Inventory {
    let Some(roots) = ScanRoots::for_machine() else {
        // Unreachable in practice: `resolve_app_data` already refuses to boot
        // without `HOME`. Logged rather than returned silently. An empty
        // inventory renders as "this machine has no installs", the opposite of
        // what this surface exists to say.
        crate::log!("[Installs] HOME is not set; cannot enumerate installs");
        return Inventory::default();
    };
    let running = RunningProcess {
        exe,
        data_dir: Some(app_data.to_path_buf()),
    };
    lucidos_installs::scan(&roots, &running)
}

/// Say what is on the machine, once, at boot.
///
/// The ordinary single-install line costs nothing. A conflict gets a block.
/// That is the state in which a user sits on an ancient engine while every
/// version number they can find looks current.
pub fn log_boot_summary(inv: &Inventory) {
    for install in &inv.installs {
        crate::log!(
            "[Installs] {} [{}]{}{}{}",
            install.name,
            install.kind.label(),
            install
                .version
                .as_deref()
                .map(|v| format!(" version {v}"))
                .unwrap_or_default(),
            install
                .port
                .map(|p| format!(" port {p}"))
                .unwrap_or_default(),
            if install.running_here {
                " (this gateway)"
            } else {
                ""
            },
        );
    }
    for conflict in &inv.conflicts {
        crate::log!(
            "[Installs] WARNING: {} Lucidos installs are configured for port {}: {}. \
             Only one can bind it, and whichever started first wins. \
             So this client may be driving an engine from a different install. \
             Settings, System, Overview lists them all.",
            conflict.installs.len(),
            conflict.port,
            conflict.installs.join(" and "),
        );
    }
}
