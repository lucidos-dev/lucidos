//! Keeping the gateway's own state directories out of file-level backup.
//!
//! Decision and threat model: ADR 0153. A Time Machine snapshot used to carry
//! the backup key and the credential database side by side. Anyone holding the
//! external disk held both halves.

use std::path::Path;

/// Mark `dir` as excluded from file-level backup, logging only the change.
///
/// Best-effort by design. The exclusion protects a secret, but failing to set
/// it must not stop a workspace from starting. A failure is logged and the
/// caller carries on. `what` names the directory for the log line.
///
/// Called at creation AND on every start. The re-check is what converges an
/// install that predates ADR 0153, and it stays silent once correct.
pub fn exclude(dir: &Path, what: &str) {
    match lucidos_file_backup_exclusion::ensure_excluded(dir) {
        Ok(outcome) if outcome.changed() => crate::log!(
            "[Gateway] excluded the {} from file-level backup: {}",
            what,
            dir.display()
        ),
        Ok(_) => {}
        Err(e) => crate::log!(
            "[Gateway] could not exclude the {} at {} from file-level backup: {}",
            what,
            dir.display(),
            e
        ),
    }
}
