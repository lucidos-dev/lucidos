use lucidos_installs::{Inventory, RunningProcess, ScanRoots};
use std::path::{Path, PathBuf};

pub fn inventory(roots: Option<&ScanRoots>, exe: Option<PathBuf>, app_data: &Path) -> Inventory {
    let Some(roots) = roots else {
        crate::log_legacy!("[Installs] HOME is not set; cannot enumerate installs");
        return Inventory::default();
    };
    let running = RunningProcess {
        exe,
        data_dir: Some(app_data.to_path_buf()),
    };
    lucidos_installs::scan(roots, &running)
}

pub fn log_boot_summary(inv: &Inventory) {
    for install in &inv.installs {
        crate::log_legacy!(
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
        crate::log_legacy!(
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
