//! What the NEXT gateway boot should do about the workspaces that were running.
//!
//! `<app-data>/.next-boot.json`, written by the packaged service as it tears the
//! stack down and consumed (and deleted) here on the way back up.
//!
//! It exists because a restart must give the user back what it took. The
//! packaged service teardown (`lucidos-app`'s `desktop::GatewayService::shutdown`)
//! stops every workspace engine and the embedded Postgres, which is right for a
//! full stop and wrong for the restart that `launchctl kickstart -k` performs:
//! [`crate::server::GatewayState::boot_all`] then re-adopts nothing (all the
//! engines are dead) and spawns only `autostart` workspaces, so a workspace the
//! user was sitting in stays stopped. Its open page cannot recover either, since
//! API traffic deliberately never lazy-starts a workspace (that guard is what
//! makes the picker's Stop button stick). On 2026-08-03 a packaged Restart left
//! the open workspace down for nine minutes until the page was reloaded by hand.
//!
//! So the teardown records the ids it stopped and this boot brings exactly those
//! back, whatever their `autostart` says: the flag governs the boot posture, not
//! whether a restart returns what it took. The same teardown runs when the
//! gateway dies and launchd respawns the service, which needs the same repair.
//!
//! Two shapes, and the second is why the record is a document rather than a bare
//! list. `{"restore": ["id", …]}` names the workspaces to bring up.
//! `{"quit": true}` is written by "Quit and Stop Background Service" BEFORE it
//! signals launchd, so the teardown that follows knows to record nothing: that
//! action is the one teardown that means *stay down*. Writing the intent first
//! makes the ordering structural instead of a race against how synchronous
//! `launchctl bootout` happens to be.
//!
//! Everything here fails soft. This sits on the boot path of the whole stack, so
//! a missing, malformed, or half-written record must mean "restore nothing", never
//! a gateway that will not start.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The record's filename under `<app-data>`. A dotfile at the app-data root,
/// alongside `.window-state.json`: transient runtime state, not user config.
/// The writer (`lucidos-app`'s `desktop.rs`) spells the same name; the two
/// crates share no code, so `next_boot_record_shape_matches_the_writer` pins the
/// exact JSON both sides agree on.
pub const NEXT_BOOT_FILE: &str = ".next-boot.json";

/// The on-disk record. Both fields default, so either shape parses and an empty
/// document is simply "nothing to do".
#[derive(Deserialize, Default)]
struct NextBoot {
    /// The last teardown was a deliberate full stop; restore nothing.
    #[serde(default)]
    quit: bool,
    /// Workspace ids whose engines the teardown stopped.
    #[serde(default)]
    restore: Vec<String>,
}

pub fn record_path(app_data: &Path) -> PathBuf {
    app_data.join(NEXT_BOOT_FILE)
}

/// Read the record and DELETE it, returning the workspace ids to bring back.
///
/// Deleting unconditionally (before parsing, and even on a read error) is what
/// makes the record one-shot: a workspace restored once must not come back on
/// every later boot, and a record we could not parse must not be retried
/// forever. Empty on a missing file, on `quit`, and on anything unreadable.
pub fn take(app_data: &Path) -> Vec<String> {
    let path = record_path(app_data);
    let raw = std::fs::read_to_string(&path);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => crate::log!("[Gateway] could not clear {}: {e}", path.display()),
    }
    let raw = match raw {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            crate::log!("[Gateway] could not read {}: {e}", path.display());
            return Vec::new();
        }
    };
    match serde_json::from_str::<NextBoot>(&raw) {
        Ok(record) if record.quit => Vec::new(),
        Ok(record) => record.restore,
        Err(e) => {
            crate::log!("[Gateway] ignoring unreadable {}: {e}", path.display());
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway app-data dir that removes itself.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("lucidos-next-boot-{tag}-{unique}"));
            std::fs::create_dir_all(&path).expect("create the temp dir");
            Self(path)
        }

        fn write(&self, body: &str) {
            std::fs::write(record_path(&self.0), body).expect("write the record");
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_restore_record_names_the_workspaces_to_bring_back() {
        let tmp = TempDir::new("restore");
        tmp.write(r#"{"restore":["myws","dev"]}"#);
        assert_eq!(take(tmp.path()), vec!["myws", "dev"]);
    }

    // The whole point of the quit shape: "Quit and Stop Background Service" must
    // not come back to life on the next launch.
    #[test]
    fn a_quit_record_restores_nothing() {
        let tmp = TempDir::new("quit");
        tmp.write(r#"{"quit":true}"#);
        assert!(take(tmp.path()).is_empty());
    }

    // One-shot: a restored workspace must not be resurrected on every later boot,
    // so the record is gone whatever it said.
    #[test]
    fn taking_the_record_consumes_it() {
        let tmp = TempDir::new("once");
        tmp.write(r#"{"restore":["myws"]}"#);
        assert_eq!(take(tmp.path()), vec!["myws"]);
        assert!(!record_path(tmp.path()).exists(), "the record must be gone");
        assert!(
            take(tmp.path()).is_empty(),
            "a second take restores nothing"
        );
    }

    // This runs on the boot path of the whole stack, so nothing here may be fatal.
    #[test]
    fn a_missing_or_unreadable_record_restores_nothing_and_is_cleared() {
        let tmp = TempDir::new("garbage");
        assert!(take(tmp.path()).is_empty(), "no record at all");

        for body in [r#"{ not json"#, "", r#"{"restore":"myws"}"#] {
            tmp.write(body);
            assert!(take(tmp.path()).is_empty(), "unreadable: {body:?}");
            assert!(
                !record_path(tmp.path()).exists(),
                "an unreadable record must not be retried forever: {body:?}",
            );
        }
    }

    // The writer lives in `lucidos-app`, which shares no code with this crate, so
    // the format can only drift silently. This pins the exact bytes it writes.
    #[test]
    fn next_boot_record_shape_matches_the_writer() {
        let tmp = TempDir::new("shape");
        tmp.write("{\"restore\":[\"myws\"]}");
        assert_eq!(take(tmp.path()), vec!["myws"]);
        tmp.write("{\"quit\":true}");
        assert!(take(tmp.path()).is_empty());
        assert_eq!(NEXT_BOOT_FILE, ".next-boot.json");
    }
}
