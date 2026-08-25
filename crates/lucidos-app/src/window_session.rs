//! Which workspaces had a window, and how big each one was.
//!
//! `<app-data>/.window-session.json`, written by the client as its windows move
//! and close, and read on the way back up.
//!
//! It exists because a relaunch must give the user back what it took. The
//! gateway already restores the workspace ENGINES a restart stopped
//! (`next_boot.rs`, in the gateway crate). Nothing restored the client's
//! WINDOWS. Only `main` is declared in `tauri.conf.json`, so only `main` comes
//! back, and it lands on whatever workspace `localStorage` remembers.
//!
//! It is keyed by workspace SLUG rather than by window label, which is the
//! other half of the same defect. `tauri-plugin-window-state` keys geometry by
//! label, and an extra window's label is `window-<n>` off a counter that resets
//! each process. That label means nothing across launches.
//!
//! Everything here fails soft. It sits on the client's boot path, so a bad
//! record must mean "restore nothing", never a client that will not start.

use crate::window_restore::Rect;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The record's filename under `<app-data>`. A dotfile at the app-data root,
/// alongside `.next-boot.json` and `.window-state.json`: transient runtime
/// state, not user config.
pub const WINDOW_SESSION_FILE: &str = ".window-session.json";

/// The window a workspace opens into, and which ones had one.
///
/// The two fields answer different questions and neither implies the other.
/// `open` is what to reopen. `geometry` is how big to make it, and it
/// deliberately OUTLIVES a window's closing. Reopening a workspace later still
/// lands at the size the user left it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSession {
    /// Workspace slugs that had a window, in the order to restore them. The
    /// first gets `main`.
    #[serde(default)]
    pub open: Vec<String>,
    /// The last known frame of each workspace's window, in PHYSICAL pixels, the
    /// units `window_restore` reasons in.
    #[serde(default)]
    pub geometry: BTreeMap<String, Rect>,
}

/// One live window, as the capture sees it.
pub struct WindowSnapshot {
    pub label: String,
    pub url: String,
    pub frame: Rect,
}

/// Has any window reached the gateway yet?
///
/// Half the gate on writing the record, and the half that rules out BOOT. Every
/// window starts on the bundled splash, and the startup geometry write arms the
/// debounced flush long before the first navigation. Writing then replaced the
/// user's arrangement with an empty one.
///
/// It asks for NAVIGATED, deliberately not for a workspace and not for visible.
/// A window on the picker is a real answer, so closing the last workspace
/// window still empties the set. And a HIDDEN window is still part of the
/// arrangement: `main` is hidden rather than closed, and the tray brings it
/// back on the workspace it was on.
///
/// The other half is `lib.rs`'s `PresentedGate`, which rules out a login start.
pub fn any_window_is_navigated(windows: &[WindowSnapshot]) -> bool {
    windows
        .iter()
        .any(|s| crate::window_target::window_is_navigated(&s.url))
}

pub fn record_path(app_data: &Path) -> PathBuf {
    app_data.join(WINDOW_SESSION_FILE)
}

/// Fold the live windows into `previous`, producing the record to write.
///
/// Pure, so the whole rule is testable without an NSWindow. Three parts.
///
/// A window counts only when it is actually ON a workspace, which
/// `window_target::window_workspace` decides from its URL. The boot splash and
/// the picker are not workspaces and must not be recorded as one.
///
/// `open` is REPLACED, so a window the user closed leaves the record.
/// `geometry` is MERGED, so a workspace with no window right now keeps its
/// remembered size.
///
/// The order is `main` first, then by label. A restore hands `open[0]` to
/// `main`, and the window map has no order of its own. Without this the same
/// two windows could swap places between launches.
pub fn capture(previous: &WindowSession, windows: &[WindowSnapshot]) -> WindowSession {
    let mut ordered: Vec<&WindowSnapshot> = windows.iter().collect();
    ordered.sort_by(|a, b| {
        let key = |s: &WindowSnapshot| (s.label != crate::MAIN_WINDOW_LABEL, s.label.clone());
        key(a).cmp(&key(b))
    });

    let mut session = WindowSession {
        open: Vec::new(),
        geometry: previous.geometry.clone(),
    };
    for snapshot in ordered {
        let Some(workspace) = crate::window_target::window_workspace(&snapshot.url) else {
            continue;
        };
        session
            .geometry
            .insert(workspace.to_string(), snapshot.frame);
        // Two windows on ONE workspace collapse to one entry. The record holds
        // no per-window identity, so a second restored window would land on top
        // of the first at the same frame.
        if !session.open.iter().any(|id| id == workspace) {
            session.open.push(workspace.to_string());
        }
    }
    session
}

/// Read the record. Empty on a missing file and on anything unreadable.
///
/// Unlike the gateway's `next_boot` record this is NOT consumed on read: it
/// describes a standing arrangement rather than a one-shot instruction, and the
/// next write is what supersedes it.
pub fn read(app_data: &Path) -> WindowSession {
    let path = record_path(app_data);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return WindowSession::default(),
        Err(e) => {
            eprintln!("[Tauri] could not read {}: {e}", path.display());
            return WindowSession::default();
        }
    };
    match serde_json::from_str::<WindowSession>(&raw) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("[Tauri] ignoring unreadable {}: {e}", path.display());
            WindowSession::default()
        }
    }
}

/// Write the record atomically (temp plus rename), so a client killed mid-write
/// leaves the previous one intact rather than a truncated file.
///
/// Best-effort and logged. Failing to record costs a relaunch its windows, and
/// must never take the client down with it.
pub fn write(app_data: &Path, session: &WindowSession) {
    let path = record_path(app_data);
    let body = match serde_json::to_string(session) {
        Ok(body) => body,
        Err(e) => {
            eprintln!("[Tauri] could not build the window-session record: {e}");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, body) {
        eprintln!("[Tauri] could not write {}: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        eprintln!("[Tauri] could not replace {}: {e}", path.display());
    }
}

/// The workspaces to restore, and the frame each one wants.
///
/// `main` takes the first, and every other entry becomes a new window. A slug
/// the record cannot justify is dropped rather than trusted: the record is a
/// file on disk, and every `window-*` webview holds the full IPC permission set
/// on the gateway origin (ADR 0028).
///
/// `restore` is the launch decision. A login start comes up menu-bar-only with
/// no window at all, so it restores nothing.
pub fn restore_plan(session: &WindowSession, restore: bool) -> Vec<(String, Option<Rect>)> {
    if !restore {
        return Vec::new();
    }
    session
        .open
        .iter()
        .filter(|id| crate::window_target::is_workspace_slug(id))
        .map(|id| (id.clone(), session.geometry.get(id).copied()))
        .collect()
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
            let path = std::env::temp_dir().join(format!("lucidos-window-session-{tag}-{unique}"));
            std::fs::create_dir_all(&path).expect("create the temp dir");
            Self(path)
        }

        fn write_raw(&self, body: &str) {
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

    fn rect(x: i64, y: i64, width: i64, height: i64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn snapshot(label: &str, url: &str, frame: Rect) -> WindowSnapshot {
        WindowSnapshot {
            label: label.to_string(),
            url: url.to_string(),
            frame,
        }
    }

    // ── capture ──────────────────────────────────────────────────────────────

    #[test]
    fn every_window_on_a_workspace_is_recorded_with_its_frame() {
        let session = capture(
            &WindowSession::default(),
            &[
                snapshot("main", "http://localhost:3210/myws/", rect(0, 0, 1200, 800)),
                snapshot(
                    "window-1",
                    "http://localhost:3210/dev/",
                    rect(100, 50, 900, 700),
                ),
            ],
        );
        assert_eq!(session.open, vec!["myws", "dev"]);
        assert_eq!(session.geometry.get("myws"), Some(&rect(0, 0, 1200, 800)));
        assert_eq!(session.geometry.get("dev"), Some(&rect(100, 50, 900, 700)));
    }

    // The picker is not a workspace, and neither is a window still on the
    // bundled boot splash. Recording either would reopen onto nothing.
    #[test]
    fn a_window_on_no_workspace_is_not_recorded() {
        let session = capture(
            &WindowSession::default(),
            &[
                snapshot("main", "http://localhost:3210/~/", rect(0, 0, 1200, 800)),
                snapshot("window-1", "tauri://localhost", rect(0, 0, 1200, 800)),
                snapshot("window-2", "http://localhost:3210/", rect(0, 0, 1200, 800)),
            ],
        );
        assert!(session.open.is_empty());
        assert!(session.geometry.is_empty());
    }

    // `main` is the window a trayed client hides and reopens, so it is the one
    // the user thinks of as theirs. Sorting also makes the order stable: the
    // window map it comes from has none.
    #[test]
    fn main_is_restored_first_and_the_rest_follow_by_label() {
        let session = capture(
            &WindowSession::default(),
            &[
                snapshot(
                    "window-2",
                    "http://localhost:3210/c/",
                    rect(0, 0, 1200, 800),
                ),
                snapshot(
                    "window-1",
                    "http://localhost:3210/b/",
                    rect(0, 0, 1200, 800),
                ),
                snapshot("main", "http://localhost:3210/a/", rect(0, 0, 1200, 800)),
            ],
        );
        assert_eq!(session.open, vec!["a", "b", "c"]);
    }

    // A stale `open` entry would resurrect a window the user deliberately shut.
    #[test]
    fn a_closed_window_leaves_the_open_set() {
        let before = capture(
            &WindowSession::default(),
            &[
                snapshot("main", "http://localhost:3210/myws/", rect(0, 0, 1200, 800)),
                snapshot(
                    "window-1",
                    "http://localhost:3210/dev/",
                    rect(0, 0, 900, 700),
                ),
            ],
        );
        let after = capture(
            &before,
            &[snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(0, 0, 1200, 800),
            )],
        );
        assert_eq!(after.open, vec!["myws"]);
    }

    // The size is remembered per WORKSPACE, not per open window: reopening one
    // later must land where the user left it, not at the default.
    #[test]
    fn a_closed_window_keeps_its_remembered_size() {
        let before = capture(
            &WindowSession::default(),
            &[
                snapshot("main", "http://localhost:3210/myws/", rect(0, 0, 1200, 800)),
                snapshot(
                    "window-1",
                    "http://localhost:3210/dev/",
                    rect(100, 50, 900, 700),
                ),
            ],
        );
        let after = capture(
            &before,
            &[snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(0, 0, 1200, 800),
            )],
        );
        assert_eq!(after.open, vec!["myws"]);
        assert_eq!(after.geometry.get("dev"), Some(&rect(100, 50, 900, 700)));
    }

    // Closing the last workspace window empties the set rather than preserving
    // it. An earlier guard keyed on "no window is on a workspace" and reopened
    // one the user had deliberately closed.
    #[test]
    fn closing_the_last_workspace_window_empties_the_open_set() {
        let before = capture(
            &WindowSession::default(),
            &[
                snapshot("main", "http://localhost:3210/myws/", rect(0, 0, 1200, 800)),
                snapshot(
                    "window-1",
                    "http://localhost:3210/dev/",
                    rect(0, 0, 900, 700),
                ),
            ],
        );
        // The user sent `main` to the picker, then closed the other window.
        let after = capture(
            &before,
            &[snapshot(
                "main",
                "http://localhost:3210/~/?pick",
                rect(0, 0, 1200, 800),
            )],
        );
        assert!(after.open.is_empty());
        // The sizes are still remembered for when either is opened again.
        assert_eq!(after.geometry.get("dev"), Some(&rect(0, 0, 900, 700)));
    }

    // ── When a capture is worth writing ──────────────────────────────────────

    #[test]
    fn a_window_on_the_gateway_is_worth_recording() {
        assert!(any_window_is_navigated(&[snapshot(
            "main",
            "http://localhost:3210/myws/",
            rect(0, 0, 1200, 800),
        )]));
    }

    // A window on the picker counts too, which is what lets `open` shrink to
    // nothing when the user closes their last workspace window.
    #[test]
    fn a_window_on_the_picker_is_worth_recording() {
        assert!(any_window_is_navigated(&[snapshot(
            "main",
            "http://localhost:3210/~/?pick",
            rect(0, 0, 1200, 800),
        )]));
    }

    // Boot. The startup geometry write arms the debounced flush while every
    // window still sits on the splash. Writing then emptied the record on
    // every launch.
    #[test]
    fn a_window_still_on_the_splash_is_not_worth_recording() {
        assert!(!any_window_is_navigated(&[snapshot(
            "main",
            "tauri://localhost",
            rect(0, 0, 1200, 800),
        )]));
        assert!(!any_window_is_navigated(&[]));
    }

    // ── read and write ───────────────────────────────────────────────────────

    #[test]
    fn a_written_record_reads_back_identical() {
        let tmp = TempDir::new("roundtrip");
        let session = capture(
            &WindowSession::default(),
            &[snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(1, 2, 1200, 800),
            )],
        );
        write(tmp.path(), &session);
        assert_eq!(read(tmp.path()), session);
    }

    // This runs on the client's boot path, so nothing here may be fatal.
    #[test]
    fn a_missing_or_unreadable_record_restores_nothing() {
        let tmp = TempDir::new("garbage");
        assert_eq!(read(tmp.path()), WindowSession::default());

        for body in [r#"{ not json"#, "", r#"{"open":"myws"}"#, "[]"] {
            tmp.write_raw(body);
            assert_eq!(
                read(tmp.path()),
                WindowSession::default(),
                "unreadable: {body:?}"
            );
        }
    }

    // Either field may be absent, so a hand-edited record still parses instead
    // of restoring nothing.
    #[test]
    fn a_partial_record_keeps_the_half_it_carries() {
        let tmp = TempDir::new("partial");
        tmp.write_raw(r#"{"open":["myws"]}"#);
        let session = read(tmp.path());
        assert_eq!(session.open, vec!["myws"]);
        assert!(session.geometry.is_empty());
    }

    // ── restore_plan ─────────────────────────────────────────────────────────

    #[test]
    fn the_plan_pairs_each_workspace_with_its_frame() {
        let session = WindowSession {
            open: vec!["myws".into(), "dev".into()],
            geometry: BTreeMap::from([("myws".to_string(), rect(1, 2, 1200, 800))]),
        };
        assert_eq!(
            restore_plan(&session, true),
            vec![
                ("myws".to_string(), Some(rect(1, 2, 1200, 800))),
                ("dev".to_string(), None),
            ]
        );
    }

    // The record is a file on disk, and every `window-*` webview holds the full
    // IPC permission set on the gateway origin (ADR 0028). A slug that is not
    // one is dropped rather than composed into a URL.
    #[test]
    fn the_plan_drops_anything_that_is_not_a_workspace_slug() {
        let session = WindowSession {
            open: vec![
                "..".into(),
                "a/b".into(),
                "MyWs".into(),
                "~".into(),
                String::new(),
                "http://evil.example".into(),
                "ok".into(),
            ],
            geometry: BTreeMap::new(),
        };
        assert_eq!(restore_plan(&session, true), vec![("ok".to_string(), None)]);
    }

    // A login start comes up menu-bar-only with no window (ADR 0072), and
    // restoring would put several on screen the user never asked for.
    #[test]
    fn a_launch_that_wants_no_window_restores_nothing() {
        let session = WindowSession {
            open: vec!["myws".into()],
            geometry: BTreeMap::new(),
        };
        assert!(restore_plan(&session, false).is_empty());
    }
}
