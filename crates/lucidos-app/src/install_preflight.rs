//! What the client says when a second Lucidos install is in the way.
//!
//! The client is the half that can rescue somebody already trapped. Their
//! gateway is old by definition, so it will never run the control-plane route
//! that lists installs. Their client, on the other hand, is current: it is what
//! they just downloaded and reinstalled, twice, wondering why nothing changed.

//! # It speaks on contention, never on coexistence
//!
//! A source checkout beside the packaged app is an ordinary setup on two
//! ports, and nothing here fires for it. What fires is two installs configured
//! for ONE port, where only one can answer and the loser is invisible. The
//! ports are the whole discriminator, so no marker file is needed.

//! # And it speaks once
//!
//! `PortConflict::fingerprint` digests the conflict this notice is ABOUT, and
//! the answer is recorded under app-data. An unchanged machine is silent on
//! every later launch. Change that conflict and the digest changes with it, so
//! a new conflict is a new thing to say.

use lucidos_installs::{Inventory, RunningProcess, ScanRoots};
use std::path::{Path, PathBuf};

/// Where the last announcement is remembered, under the client's app-data.
const ACK_FILE: &str = "config/install-conflict-ack";

/// A message the user has not been shown yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictNotice {
    /// What the message is about, for the once-per-state rule.
    pub fingerprint: String,
    /// The whole thing, ready for a native dialog.
    pub body: String,
}

/// Compose the warning for a machine whose installs contend, or `None`.
///
/// Pure, so the wording and the trigger are both testable. `serving_release` is
/// what the gateway on `port` answers with, when it answers at all. It turns a
/// general warning into the exact sentence the user needs.
pub fn conflict_notice(
    inventory: &Inventory,
    port: u16,
    serving_release: Option<&str>,
    app_version: &str,
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
    if let Some(running) = serving_release.filter(|r| *r != app_version) {
        body.push_str(&format!(
            "\nRight now port {port} is answered by Lucidos {running}, \
             and this app is {app_version}.\n"
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

/// Is this worth saying, given what was said last time?
///
/// An empty fingerprint is nothing to announce. Anything else is announced
/// exactly once, and again only when the machine changes underneath it.
pub fn should_announce(fingerprint: &str, acknowledged: Option<&str>) -> bool {
    !fingerprint.is_empty() && acknowledged != Some(fingerprint)
}

/// Scan, decide, and record. `Some` means the caller owes the user a dialog.
///
/// The record is written before the dialog is shown rather than after it is
/// dismissed. The promise is to say it once, and a user who dismisses a warning
/// without reading it has still been told.
pub fn take_notice(app_data: &Path, port: u16, app_version: &str) -> Option<ConflictNotice> {
    let roots = ScanRoots::for_machine()?;
    let running = RunningProcess {
        exe: std::env::current_exe().ok(),
        data_dir: Some(app_data.to_path_buf()),
    };
    let inventory = lucidos_installs::scan(&roots, &running);
    let serving = serving_release(port);
    let notice = conflict_notice(&inventory, port, serving.as_deref(), app_version)?;
    if !should_announce(&notice.fingerprint, read_ack(app_data).as_deref()) {
        return None;
    }
    write_ack(app_data, &notice.fingerprint);
    Some(notice)
}

/// The release the gateway on `port` reports, from its own health endpoint.
///
/// `None` covers a gateway too old to carry the field and one that did not
/// answer. Both mean "unknown", which the notice renders by saying nothing.
fn serving_release(port: u16) -> Option<String> {
    let body = crate::desktop::gateway_body(port, "GET", "/~/api/v1/health")?;
    parse_release(&body)
}

/// Read `release` out of a gateway health body. Pure, so the tolerance for an
/// older gateway's shape is pinned by a test rather than by a live one.
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

/// Best-effort. A machine that cannot record the acknowledgement warns again
/// next launch, which is the safe direction to fail in.
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
