use lucidos_installs::Inventory;
use std::path::{Path, PathBuf};

const ACK_FILE: &str = "config/install-conflict-ack";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictNotice {
    pub fingerprint: String,
    pub body: String,
}

pub fn conflict_notice(
    inventory: &Inventory,
    port: u16,
    serving_release: Option<&str>,
    app_release: &str,
) -> Option<ConflictNotice> {
    let conflict = inventory.conflicts.iter().find(|c| c.port == port)?;
    let mut body = format!(
        "{} Lucidos installs are set up to use port {}:\n\n",
        conflict.installs.len(),
        conflict.port
    );
    for install in &inventory.installs {
        if !conflict.installs.contains(&install.name) {
            continue;
        }
        let version = install
            .version
            .as_deref()
            .map(|v| format!(" version {v}"))
            .unwrap_or_else(|| " version unknown".to_string());
        body.push_str(&format!(
            "  \u{2022} {} [{}]{version}\n",
            install.name,
            install.kind.label()
        ));
    }
    body.push_str(
        "\nOnly one of them can answer on that port, and whichever started \
         first wins. So this app may be driving the other install's engine.\n",
    );
    if let Some(running) = serving_release.filter(|r| *r != app_release) {
        body.push_str(&format!(
            "\nRight now port {port} is answered by Lucidos {running}, \
             and this app is {app_release}.\n"
        ));
    }
    body.push_str(
        "\nSettings, then System, then Overview lists every install found, \
         with the command that removes one.",
    );
    Some(ConflictNotice {
        fingerprint: conflict.fingerprint(),
        body,
    })
}

pub fn should_announce(fingerprint: &str, acknowledged: Option<&str>) -> bool {
    !fingerprint.is_empty() && acknowledged != Some(fingerprint)
}

pub fn take_notice(app_data: &Path, port: u16, app_release: &str) -> Option<ConflictNotice> {
    let inventory = lucidos_installs::scan_this_machine(Some(app_data.to_path_buf()));
    let serving = serving_release(port);
    let notice = conflict_notice(&inventory, port, serving.as_deref(), app_release)?;
    if !should_announce(&notice.fingerprint, read_ack(app_data).as_deref()) {
        return None;
    }
    write_ack(app_data, &notice.fingerprint);
    Some(notice)
}

fn serving_release(port: u16) -> Option<String> {
    let body =
        crate::desktop::gateway_body(port, "GET", crate::desktop::GATEWAY_HEALTH_PATH, None)?;
    parse_release(&body)
}

pub fn parse_release(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let release = json.get("release")?.as_str()?.trim();
    (!release.is_empty()).then(|| release.to_string())
}

fn ack_path(app_data: &Path) -> PathBuf {
    app_data.join(ACK_FILE)
}

fn read_ack(app_data: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(ack_path(app_data)).ok()?;
    Some(raw.trim().to_string())
}

fn write_ack(app_data: &Path, fingerprint: &str) {
    let path = ack_path(app_data);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, format!("{fingerprint}\n")) {
        eprintln!(
            "[desktop] could not record the install-conflict notice at {}: {e}",
            path.display()
        );
    }
}

#[cfg(test)]
#[path = "install_preflight_tests.rs"]
mod tests;
