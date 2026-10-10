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
//! each process. That label means nothing across launches, which is why the
//! plugin is confined to `main` (`window_persist::plugin_tracks`).
//!
//! Everything here fails soft. It sits on the client's boot path, so a bad
//! record must mean "restore nothing", never a client that will not start.

use crate::window_restore::{RememberedFrame, Whereabouts};
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSession {
    /// Workspace slugs that had a window, in the order to restore them. The
    /// first gets `main`.
    #[serde(default)]
    pub open: Vec<String>,
    /// The last known frame of each workspace's window, in LOGICAL points, the
    /// units `window_restore` reasons in. See [`FrameUnits`].
    #[serde(default)]
    pub geometry: BTreeMap<String, RememberedFrame>,
    /// The space `geometry` is written in, so a record this build did not write
    /// cannot be read as if it had.
    #[serde(default = "FrameUnits::unmarked")]
    pub units: FrameUnits,
}

impl Default for WindowSession {
    /// Nothing to restore, and nothing that could be misread. An empty record
    /// is in points because every record this build writes is. An unmarked
    /// FILE is a different fact, and [`FrameUnits::unmarked`] carries that one.
    fn default() -> Self {
        Self {
            open: Vec::new(),
            geometry: BTreeMap::new(),
            units: FrameUnits::LogicalPoints,
        }
    }
}

/// The coordinate space a record's frames are in.
///
/// A marker rather than a version number, because the only thing that ever
/// changed is what the numbers MEAN. A record written before ADR 0173 holds
/// physical pixels, which are not one space across monitors at different scale
/// factors. Such a rect cannot be converted after the fact: that needs the scale
/// factor of the display it was captured on, which the record never held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrameUnits {
    /// The pre-fix space, whose frames [`in_points`] drops.
    PhysicalPixels,
    /// Logical points in the macOS global desktop space, what every frame this
    /// build writes means.
    LogicalPoints,
}

impl FrameUnits {
    /// What a record naming no space is written in. It predates ADR 0173, so
    /// its frames are physical pixels and [`in_points`] drops them.
    fn unmarked() -> Self {
        Self::PhysicalPixels
    }
}

/// One live window, as the capture sees it.
pub struct WindowSnapshot {
    pub label: String,
    pub url: String,
    pub frame: crate::window_restore::Rect,
    /// Where `frame` sits on the desk: the display to anchor it to, or no
    /// screen at all. An orphaned frame is held back like a rescue (ADR 0269).
    pub whereabouts: Whereabouts,
    /// Is `frame` a correction the CLIENT made, rather than where the user put
    /// the window?
    ///
    /// A window is rescued when the desk could not hold the frame it was
    /// remembered at. That includes a desk which was simply not all there yet.
    /// Recording the rescue answers a question the user never asked, and loses
    /// the arrangement they did choose. So [`capture`] keeps what the record
    /// holds while this is true. ADR 0215 has the rest, and
    /// `window_restore::is_wearing_a_rescue` decides when it stops being true.
    pub rescued: bool,
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
/// The other half is `window_persist.rs`'s `PresentedGate`, which rules out a login start.
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
        crate::app_window::window_order_key(&a.label)
            .cmp(&crate::app_window::window_order_key(&b.label))
    });

    let mut session = WindowSession {
        open: Vec::new(),
        geometry: previous.geometry.clone(),
        // Every frame here came off a live window through
        // `Rect::from_physical`, so the record can say so. `read` is what makes
        // `previous` safe to merge: it drops frames in any other space.
        units: FrameUnits::LogicalPoints,
    };
    for snapshot in ordered {
        let Some(workspace) = crate::window_target::window_workspace(&snapshot.url) else {
            continue;
        };
        // A rescued window keeps whatever the record already held for its
        // workspace, because a frame the client chose is not an arrangement
        // (ADR 0215). Nor is one whose title bar is on no screen, since the
        // system put it there (ADR 0269). With nothing held there is nothing
        // better to keep, so the frame goes in and the workspace at least
        // reopens somewhere.
        let (not_chosen, anchor) = match &snapshot.whereabouts {
            Whereabouts::Orphaned => (true, None),
            Whereabouts::OnScreen(anchor) => (snapshot.rescued, anchor.clone()),
        };
        let keep_previous = not_chosen && session.geometry.contains_key(workspace);
        if !keep_previous {
            session.geometry.insert(
                workspace.to_string(),
                RememberedFrame::new(snapshot.frame, anchor),
            );
        }
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
        Ok(session) => in_points(session),
        Err(e) => {
            eprintln!("[Tauri] ignoring unreadable {}: {e}", path.display());
            WindowSession::default()
        }
    }
}

/// Drop frames this build cannot interpret, and keep everything it can.
///
/// A record from before ADR 0173 holds physical pixels. Restoring one on a
/// mixed-DPI desk puts the window at the wrong size in the wrong place, which is
/// the whole bug. `open` is untouched, so the launch after the upgrade still
/// reopens every workspace that had a window. Each lands at the declared default
/// until it is moved once, and the next capture records it in points.
fn in_points(session: WindowSession) -> WindowSession {
    if session.units == FrameUnits::LogicalPoints {
        return session;
    }
    WindowSession {
        open: session.open,
        geometry: BTreeMap::new(),
        units: FrameUnits::LogicalPoints,
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

/// The frame recorded for the workspace `url` serves, when one is remembered.
///
/// The exact mirror of what [`capture`] wrote. That keys each frame by
/// `window_target::window_workspace` of the window's URL. This reads it back by
/// the same parse, of the URL a window is about to load. So the two cannot key
/// differently, which a caller deriving a slug some other way could.
///
/// `None` for a URL on no workspace (the picker, the boot splash) and for a
/// workspace nothing is remembered about. Both mean the same thing to a caller:
/// build the window at the declared default.
pub fn frame_for_url(session: &WindowSession, url: &str) -> Option<RememberedFrame> {
    let workspace = crate::window_target::window_workspace(url)?;
    session.geometry.get(workspace).cloned()
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
pub fn restore_plan(
    session: &WindowSession,
    restore: bool,
) -> Vec<(String, Option<RememberedFrame>)> {
    if !restore {
        return Vec::new();
    }
    session
        .open
        .iter()
        .filter(|id| crate::window_target::is_workspace_slug(id))
        .map(|id| (id.clone(), session.geometry.get(id).cloned()))
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

    use crate::window_restore::Rect;

    fn rect(x: i64, y: i64, width: i64, height: i64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// What an older build, or an unreadable desk, records for `frame`.
    fn held(frame: Rect) -> RememberedFrame {
        RememberedFrame::unanchored(frame)
    }

    /// The frame recorded for `workspace`, if any.
    fn frame_of(session: &WindowSession, workspace: &str) -> Option<RememberedFrame> {
        session.geometry.get(workspace).cloned()
    }

    /// A window wearing a frame the USER chose, which is nearly every window.
    fn snapshot(label: &str, url: &str, frame: Rect) -> WindowSnapshot {
        WindowSnapshot {
            label: label.to_string(),
            url: url.to_string(),
            frame,
            whereabouts: Whereabouts::OnScreen(None),
            rescued: false,
        }
    }

    /// A window whose title bar is on no attached screen.
    fn orphaned_snapshot(label: &str, url: &str, frame: Rect) -> WindowSnapshot {
        WindowSnapshot {
            whereabouts: Whereabouts::Orphaned,
            ..snapshot(label, url, frame)
        }
    }

    /// A window wearing a frame the CLAMP chose, because the desk could not
    /// hold the one it was remembered at.
    fn rescued_snapshot(label: &str, url: &str, frame: Rect) -> WindowSnapshot {
        WindowSnapshot {
            rescued: true,
            ..snapshot(label, url, frame)
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
        assert_eq!(
            frame_of(&session, "myws"),
            Some(held(rect(0, 0, 1200, 800)))
        );
        assert_eq!(
            frame_of(&session, "dev"),
            Some(held(rect(100, 50, 900, 700)))
        );
    }

    // ── a correction is not an arrangement (ADR 0215) ────────────────────────

    // The reported defect. A launch that cannot see the display a window was
    // remembered on rescues it onto one it can. The debounced flush recorded
    // that within a second. The arrangement was then gone for good, and the
    // window never went home when the display came back.
    #[test]
    fn a_rescued_window_keeps_the_frame_the_record_already_held() {
        let before = capture(
            &WindowSession::default(),
            &[snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(643, -191, 1728, 1084),
            )],
        );
        let after = capture(
            &before,
            &[rescued_snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(643, 30, 1728, 1084),
            )],
        );
        assert_eq!(
            frame_of(&after, "myws"),
            Some(held(rect(643, -191, 1728, 1084)))
        );
        // Still open, and still first. Only the frame is held back.
        assert_eq!(after.open, vec!["myws"]);
    }

    // The other half, and the one that matters more. A held frame outliving
    // the user's own gesture is a window that cannot be re-arranged at all.
    #[test]
    fn a_window_the_user_moved_records_where_they_put_it() {
        let before = capture(
            &WindowSession::default(),
            &[snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(643, -191, 1728, 1084),
            )],
        );
        let after = capture(
            &before,
            &[snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(200, 100, 1200, 800),
            )],
        );
        assert_eq!(
            frame_of(&after, "myws"),
            Some(held(rect(200, 100, 1200, 800)))
        );
    }

    // Nothing better to hold, so the correction goes in. Dropping the frame
    // instead would reopen the workspace at the declared default.
    #[test]
    fn a_rescued_window_with_no_remembered_frame_records_the_correction() {
        let session = capture(
            &WindowSession::default(),
            &[rescued_snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(643, 30, 1728, 1084),
            )],
        );
        assert_eq!(
            frame_of(&session, "myws"),
            Some(held(rect(643, 30, 1728, 1084)))
        );
    }

    // One rescued window must not hold back a workspace it is not on.
    #[test]
    fn a_rescue_holds_back_its_own_workspace_and_no_other() {
        let before = capture(
            &WindowSession::default(),
            &[
                snapshot("main", "http://localhost:3210/myws/", rect(0, 0, 1200, 800)),
                snapshot(
                    "window-1",
                    "http://localhost:3210/dev/",
                    rect(50, 50, 900, 700),
                ),
            ],
        );
        let after = capture(
            &before,
            &[
                rescued_snapshot(
                    "main",
                    "http://localhost:3210/myws/",
                    rect(300, 30, 800, 600),
                ),
                snapshot(
                    "window-1",
                    "http://localhost:3210/dev/",
                    rect(70, 70, 900, 700),
                ),
            ],
        );
        assert_eq!(frame_of(&after, "myws"), Some(held(rect(0, 0, 1200, 800))));
        assert_eq!(frame_of(&after, "dev"), Some(held(rect(70, 70, 900, 700))));
    }

    // ── an orphaned frame is not an arrangement (ADR 0269) ───────────────────

    // The reported defect. An unplug left a window at 643,-191, above every
    // screen, the flush recorded it, and every later launch rescued it onto
    // the primary display.
    #[test]
    fn an_orphaned_window_keeps_the_frame_the_record_already_held() {
        let before = capture(
            &WindowSession::default(),
            &[snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(1763, 1473, 1728, 1084),
            )],
        );
        let after = capture(
            &before,
            &[orphaned_snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(643, -191, 1728, 1084),
            )],
        );
        assert_eq!(
            frame_of(&after, "myws"),
            Some(held(rect(1763, 1473, 1728, 1084)))
        );
        assert_eq!(after.open, vec!["myws"]);
    }

    // Nothing better to hold, so the frame goes in, the same as a rescue.
    #[test]
    fn an_orphaned_window_with_no_remembered_frame_is_recorded() {
        let session = capture(
            &WindowSession::default(),
            &[orphaned_snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(643, -191, 1728, 1084),
            )],
        );
        assert_eq!(
            frame_of(&session, "myws"),
            Some(held(rect(643, -191, 1728, 1084)))
        );
    }

    // A record from before ADR 0269 has no `display` key, and must read as it
    // always did: each frame unanchored, restoring exactly where it says.
    #[test]
    fn a_record_with_no_anchors_reads_as_before() {
        let tmp = TempDir::new("unanchored");
        tmp.write_raw(
            r#"{"open":["myws","dev"],"units":"logical-points","geometry":{
                "myws":{"x":0,"y":356,"width":1280,"height":1084},
                "dev":{"x":1763,"y":1473,"width":1728,"height":1084}}}"#,
        );
        let session = read(tmp.path());
        assert_eq!(session.open, vec!["myws", "dev"]);
        assert_eq!(
            frame_of(&session, "myws"),
            Some(held(rect(0, 356, 1280, 1084)))
        );
        assert_eq!(
            frame_of(&session, "dev"),
            Some(held(rect(1763, 1473, 1728, 1084)))
        );
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
        assert_eq!(frame_of(&after, "dev"), Some(held(rect(100, 50, 900, 700))));
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
        assert_eq!(frame_of(&after, "dev"), Some(held(rect(0, 0, 900, 700))));
    }

    // Close to Menu Bar HIDES every window rather than destroying any, so the
    // window list a park leaves behind is the same list. Visibility is
    // deliberately not an input here: a hidden window is still part of the
    // arrangement, and `main` has always been hidden rather than closed. Add a
    // visibility filter and a park silently forgets every window but one.
    #[test]
    fn a_parked_window_is_still_part_of_the_arrangement() {
        let live = [
            snapshot("main", "http://localhost:3210/myws/", rect(0, 0, 1200, 800)),
            snapshot(
                "window-1",
                "http://localhost:3210/dev/",
                rect(100, 50, 900, 700),
            ),
        ];
        let before = capture(&WindowSession::default(), &live);
        assert_eq!(capture(&before, &live), before);
        assert_eq!(before.open, vec!["myws", "dev"]);
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
        // Stamped, or the read below would drop the frame it just wrote and
        // every launch would open at the default size.
        assert_eq!(session.units, FrameUnits::LogicalPoints);
        write(tmp.path(), &session);
        assert_eq!(read(tmp.path()), session);
    }

    // A record from before ADR 0173 holds physical pixels, which cannot be
    // converted: that needs the scale factor of the display each frame was
    // captured on, and the record never held it. The frames go, `open` stays,
    // so the launch after the upgrade still reopens every workspace.
    #[test]
    fn a_legacy_record_keeps_its_open_set_and_drops_its_frames() {
        let tmp = TempDir::new("legacy");
        tmp.write_raw(
            r#"{"open":["myws","dev"],
                "geometry":{"myws":{"x":4050,"y":3250,"width":3456,"height":2168}}}"#,
        );
        let session = read(tmp.path());
        assert_eq!(session.open, vec!["myws", "dev"]);
        assert!(session.geometry.is_empty());
        // In points now, vacuously, so a capture can merge it without checking.
        assert_eq!(session.units, FrameUnits::LogicalPoints);
    }

    #[test]
    fn a_stamped_record_keeps_its_frames() {
        let tmp = TempDir::new("stamped");
        tmp.write_raw(
            r#"{"open":["myws"],"units":"logical-points",
                "geometry":{"myws":{"x":100,"y":200,"width":800,"height":600}}}"#,
        );
        assert_eq!(
            frame_of(&read(tmp.path()), "myws"),
            Some(held(rect(100, 200, 800, 600)))
        );
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

    // ── frame_for_url ────────────────────────────────────────────────────────

    // The reason `geometry` is merged rather than replaced: a workspace with no
    // window right now is exactly the one a reopen has to size.
    #[test]
    fn a_closed_workspace_is_reopened_at_the_frame_it_was_left() {
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
        // Both, whether or not a window is still on them: the caller only ever
        // asks about a workspace nothing is showing.
        assert_eq!(
            frame_for_url(&session, "http://localhost:3210/dev/"),
            Some(held(rect(100, 50, 900, 700)))
        );
        assert_eq!(
            frame_for_url(&session, "http://localhost:3210/myws/"),
            Some(held(rect(0, 0, 1200, 800)))
        );
        // The landing fragment and a deep link ride the same URL, and neither is
        // part of the key.
        assert_eq!(
            frame_for_url(&session, "http://localhost:3210/dev/#notifications"),
            Some(held(rect(100, 50, 900, 700)))
        );
    }

    // Nothing remembered, and nothing that is a workspace at all. Both hand the
    // caller the declared default rather than another workspace's frame.
    #[test]
    fn a_url_with_no_recorded_frame_asks_for_the_default() {
        let session = WindowSession {
            open: vec!["myws".into()],
            geometry: BTreeMap::from([("myws".to_string(), held(rect(1, 2, 1200, 800)))]),
            units: FrameUnits::LogicalPoints,
        };
        for url in [
            "http://localhost:3210/dev/",
            "http://localhost:3210/~/?pick",
            "http://localhost:3210/",
            "tauri://localhost",
            "",
        ] {
            assert_eq!(frame_for_url(&session, url), None, "{url:?}");
        }
        assert_eq!(
            frame_for_url(&WindowSession::default(), "http://localhost:3210/myws/"),
            None
        );
    }

    // ── restore_plan ─────────────────────────────────────────────────────────

    #[test]
    fn the_plan_pairs_each_workspace_with_its_frame() {
        let session = WindowSession {
            open: vec!["myws".into(), "dev".into()],
            geometry: BTreeMap::from([("myws".to_string(), held(rect(1, 2, 1200, 800)))]),
            units: FrameUnits::LogicalPoints,
        };
        assert_eq!(
            restore_plan(&session, true),
            vec![
                ("myws".to_string(), Some(held(rect(1, 2, 1200, 800)))),
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
            units: FrameUnits::LogicalPoints,
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
            units: FrameUnits::LogicalPoints,
        };
        assert!(restore_plan(&session, false).is_empty());
    }
}
