//! One remembered value per file, under `<app_data>/config/`.
//!
//! Two things the packaged client learns from its frontend have to survive to
//! the next cold launch, because both are painted BEFORE any frontend can
//! report: the window-background tint (`config/titlebar-color`) and the header
//! bar's height, which is where the macOS traffic lights are centred
//! (`config/titlebar-bar-height`). Each is a bare scalar with no schema, and
//! each was written out with its own path helper, its own read and its own
//! skip-if-unchanged write until the second one arrived and made the shape a
//! duplicate.
//!
//! Deliberately plumbing ONLY. Every value's meaning stays with its owner: what
//! counts as valid, what the compiled default is, and what to do with a file
//! that no longer parses are all decisions the caller makes on the `Option<String>`
//! this hands back. That split is why one module can serve a colour and a
//! length without either one's rules leaking into the other.
//!
//! Beside `config/engine-port`, `config/workspaces.json` and
//! `config/device-ids.json` in the same directory, so a delete-data uninstall
//! (`desktop::support_data_paths`) forgets all of them together.

use tauri::Manager;

/// Where `file` lives, or `None` when the app data dir cannot be resolved (which
/// is not a state a packaged launch reaches, and is not worth failing a cosmetic
/// hint over anywhere else).
pub(crate) fn path(app: &tauri::AppHandle, file: &str) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("config").join(file))
}

/// The remembered value, trimmed, or `None` when the file is missing or
/// unreadable. **First-run absence is normal and says nothing**, so every
/// failure collapses to the same absent value rather than a log line: these are
/// hints for one frame of one launch, and the caller has a compiled default.
pub(crate) fn read(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|raw| raw.trim().to_string())
}

/// Remember `value` under `file` for the next launch, unless it is already what
/// is on disk.
///
/// **`value` must be the canonical form the caller will compare against**, i.e.
/// trimmed, because [`read`] trims what it returns. Writing a padded string
/// would leave a value that never compares equal to what is read back, so the
/// skip below would never hit and every apply would rewrite the file. Both
/// callers re-fire far more often than their value changes (`applyTheme` runs on
/// every system-appearance change, the traffic-light push on every preferences
/// load), so agreeing with disk is the common case and the skip is what keeps
/// this off the disk entirely.
///
/// Best-effort: a failure is logged with `what` naming the value and then
/// dropped, never surfaced. The cost of losing a write is one slightly wrong
/// frame at the start of the next launch, which the frontend corrects as soon as
/// it reports in.
pub(crate) fn write_if_changed(app: &tauri::AppHandle, file: &str, value: &str, what: &str) {
    let Some(path) = path(app, file) else {
        return;
    };
    if read(&path).as_deref() == Some(value) {
        return;
    }
    let Some(dir) = path.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, value)) {
        eprintln!("[Tauri] Failed to remember the {what}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tempdir of this module's own, since the helpers under test take an
    /// `AppHandle` for the path and a bare `&Path` for the read.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lucidos-config-scalar-{}-{}-{:?}",
            std::process::id(),
            name,
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_absent_file_reads_as_absent_rather_than_as_an_error() {
        let dir = scratch("absent");
        assert_eq!(read(&dir.join("nothing-here")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stored_value_reads_back_trimmed() {
        // The write side is expected to store the canonical form, but a
        // hand-edited file picks up a trailing newline from any editor, and the
        // caller's parse must not have to know that.
        let dir = scratch("trim");
        let file = dir.join("value");
        std::fs::write(&file, "  #15549e\n").unwrap();
        assert_eq!(read(&file).as_deref(), Some("#15549e"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
