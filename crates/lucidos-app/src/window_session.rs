use crate::window_restore::{RememberedFrame, Whereabouts};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const WINDOW_SESSION_FILE: &str = ".window-session.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSession {
    #[serde(default)]
    pub open: Vec<OpenWindow>,
    #[serde(default)]
    pub geometry: BTreeMap<String, RememberedFrame>,
    #[serde(default = "FrameUnits::unmarked")]
    pub units: FrameUnits,
}

impl Default for WindowSession {
    fn default() -> Self {
        Self {
            open: Vec::new(),
            geometry: BTreeMap::new(),
            units: FrameUnits::LogicalPoints,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RecordedWindow")]
pub struct OpenWindow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<RememberedFrame>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RecordedWindow {
    Slug(String),
    Window {
        #[serde(default)]
        label: Option<String>,
        workspace: String,
        #[serde(default)]
        frame: Option<RememberedFrame>,
    },
}

impl From<RecordedWindow> for OpenWindow {
    fn from(recorded: RecordedWindow) -> Self {
        match recorded {
            RecordedWindow::Slug(workspace) => Self {
                label: None,
                workspace,
                frame: None,
            },
            RecordedWindow::Window {
                label,
                workspace,
                frame,
            } => Self {
                label,
                workspace,
                frame,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrameUnits {
    PhysicalPixels,
    LogicalPoints,
}

impl FrameUnits {
    fn unmarked() -> Self {
        Self::PhysicalPixels
    }
}

pub struct WindowSnapshot {
    pub label: String,
    pub url: String,
    pub frame: crate::window_restore::Rect,
    pub whereabouts: Whereabouts,
    pub rescued: bool,
}

pub fn any_window_is_navigated(windows: &[WindowSnapshot]) -> bool {
    windows
        .iter()
        .any(|s| crate::window_target::window_is_navigated(&s.url))
}

pub fn record_path(app_data: &Path) -> PathBuf {
    app_data.join(WINDOW_SESSION_FILE)
}

pub fn capture(previous: &WindowSession, windows: &[WindowSnapshot]) -> WindowSession {
    let mut ordered: Vec<&WindowSnapshot> = windows.iter().collect();
    ordered.sort_by(|a, b| {
        crate::app_window::window_order_key(&a.label)
            .cmp(&crate::app_window::window_order_key(&b.label))
    });

    let mut session = WindowSession {
        open: Vec::new(),
        geometry: previous.geometry.clone(),
        units: FrameUnits::LogicalPoints,
    };
    for snapshot in ordered {
        let Some(workspace) = crate::window_target::window_workspace(&snapshot.url) else {
            continue;
        };
        let (not_chosen, anchor) = match &snapshot.whereabouts {
            Whereabouts::Orphaned => (true, None),
            Whereabouts::OnScreen(anchor) => (snapshot.rescued, anchor.clone()),
        };
        let chosen = RememberedFrame::new(snapshot.frame, anchor);
        let keep_previous = not_chosen && session.geometry.contains_key(workspace);
        if !keep_previous {
            session
                .geometry
                .insert(workspace.to_string(), chosen.clone());
        }
        let held = previous
            .open
            .iter()
            .find(|entry| entry.label.as_deref() == Some(snapshot.label.as_str()))
            .and_then(|entry| entry.frame.clone());
        let frame = match held {
            Some(held) if not_chosen => held,
            _ => chosen,
        };
        session.open.push(OpenWindow {
            label: Some(snapshot.label.clone()),
            workspace: workspace.to_string(),
            frame: Some(frame),
        });
    }
    session
}

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

fn in_points(session: WindowSession) -> WindowSession {
    if session.units == FrameUnits::LogicalPoints {
        return session;
    }
    WindowSession {
        open: session
            .open
            .into_iter()
            .map(|entry| OpenWindow {
                frame: None,
                ..entry
            })
            .collect(),
        geometry: BTreeMap::new(),
        units: FrameUnits::LogicalPoints,
    }
}

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

pub fn frame_for_url(session: &WindowSession, url: &str) -> Option<RememberedFrame> {
    let workspace = crate::window_target::window_workspace(url)?;
    session.geometry.get(workspace).cloned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredWindow {
    pub label: Option<String>,
    pub workspace: String,
    pub frame: Option<RememberedFrame>,
}

impl RestoredWindow {
    pub fn main_may_take(&self) -> bool {
        self.label
            .as_deref()
            .is_none_or(|label| label == crate::app_window::MAIN_WINDOW_LABEL)
    }
}

pub fn restore_plan(session: &WindowSession, restore: bool) -> Vec<RestoredWindow> {
    if !restore {
        return Vec::new();
    }
    let mut taken: Vec<&str> = Vec::new();
    let mut plan: Vec<RestoredWindow> = Vec::new();
    for entry in &session.open {
        if !crate::window_target::is_workspace_route(&entry.workspace) {
            continue;
        }
        let label = entry
            .label
            .as_deref()
            .filter(|label| crate::app_window::is_restorable_label(label))
            .filter(|label| !taken.contains(label));
        taken.extend(label);
        plan.push(RestoredWindow {
            label: label.map(str::to_string),
            workspace: entry.workspace.clone(),
            frame: entry
                .frame
                .clone()
                .or_else(|| session.geometry.get(&entry.workspace).cloned()),
        });
    }
    plan.sort_by_key(|window| {
        window.label.as_deref() != Some(crate::app_window::MAIN_WINDOW_LABEL)
    });
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_raw(app_data: &Path, body: &str) {
        std::fs::write(record_path(app_data), body).expect("write the record");
    }

    use crate::window_restore::Rect;

    fn workspaces(session: &WindowSession) -> Vec<&str> {
        session.open.iter().map(|w| w.workspace.as_str()).collect()
    }

    fn unlabelled(workspaces: &[&str]) -> Vec<OpenWindow> {
        workspaces
            .iter()
            .map(|id| OpenWindow {
                label: None,
                workspace: id.to_string(),
                frame: None,
            })
            .collect()
    }

    fn fresh(workspace: &str, frame: Option<RememberedFrame>) -> RestoredWindow {
        RestoredWindow {
            label: None,
            workspace: workspace.to_string(),
            frame,
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

    fn held(frame: Rect) -> RememberedFrame {
        RememberedFrame::unanchored(frame)
    }

    fn frame_of(session: &WindowSession, workspace: &str) -> Option<RememberedFrame> {
        session.geometry.get(workspace).cloned()
    }

    fn snapshot(label: &str, url: &str, frame: Rect) -> WindowSnapshot {
        WindowSnapshot {
            label: label.to_string(),
            url: url.to_string(),
            frame,
            whereabouts: Whereabouts::OnScreen(None),
            rescued: false,
        }
    }

    fn orphaned_snapshot(label: &str, url: &str, frame: Rect) -> WindowSnapshot {
        WindowSnapshot {
            whereabouts: Whereabouts::Orphaned,
            ..snapshot(label, url, frame)
        }
    }

    fn rescued_snapshot(label: &str, url: &str, frame: Rect) -> WindowSnapshot {
        WindowSnapshot {
            rescued: true,
            ..snapshot(label, url, frame)
        }
    }

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
        assert_eq!(workspaces(&session), vec!["myws", "dev"]);
        assert_eq!(
            frame_of(&session, "myws"),
            Some(held(rect(0, 0, 1200, 800)))
        );
        assert_eq!(
            frame_of(&session, "dev"),
            Some(held(rect(100, 50, 900, 700)))
        );
    }

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
        assert_eq!(workspaces(&after), vec!["myws"]);
    }

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
        assert_eq!(workspaces(&after), vec!["myws"]);
    }

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

    #[test]
    fn a_record_with_no_anchors_reads_as_before() {
        let tmp = tempfile::tempdir().unwrap();
        write_raw(
            tmp.path(),
            r#"{"open":["myws","dev"],"units":"logical-points","geometry":{
                "myws":{"x":0,"y":356,"width":1280,"height":1084},
                "dev":{"x":1763,"y":1473,"width":1728,"height":1084}}}"#,
        );
        let session = read(tmp.path());
        assert_eq!(workspaces(&session), vec!["myws", "dev"]);
        assert_eq!(
            frame_of(&session, "myws"),
            Some(held(rect(0, 356, 1280, 1084)))
        );
        assert_eq!(
            frame_of(&session, "dev"),
            Some(held(rect(1763, 1473, 1728, 1084)))
        );
    }

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
        assert!(workspaces(&session).is_empty());
        assert!(session.geometry.is_empty());
    }

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
        assert_eq!(workspaces(&session), vec!["a", "b", "c"]);
    }

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
        assert_eq!(workspaces(&after), vec!["myws"]);
    }

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
        assert_eq!(workspaces(&after), vec!["myws"]);
        assert_eq!(frame_of(&after, "dev"), Some(held(rect(100, 50, 900, 700))));
    }

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
        let after = capture(
            &before,
            &[snapshot(
                "main",
                "http://localhost:3210/~/?pick",
                rect(0, 0, 1200, 800),
            )],
        );
        assert!(workspaces(&after).is_empty());
        assert_eq!(frame_of(&after, "dev"), Some(held(rect(0, 0, 900, 700))));
    }

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
        assert_eq!(workspaces(&before), vec!["myws", "dev"]);
    }

    #[test]
    fn a_window_on_the_gateway_is_worth_recording() {
        assert!(any_window_is_navigated(&[snapshot(
            "main",
            "http://localhost:3210/myws/",
            rect(0, 0, 1200, 800),
        )]));
    }

    #[test]
    fn a_window_on_the_picker_is_worth_recording() {
        assert!(any_window_is_navigated(&[snapshot(
            "main",
            "http://localhost:3210/~/?pick",
            rect(0, 0, 1200, 800),
        )]));
    }

    #[test]
    fn a_window_still_on_the_splash_is_not_worth_recording() {
        assert!(!any_window_is_navigated(&[snapshot(
            "main",
            "tauri://localhost",
            rect(0, 0, 1200, 800),
        )]));
        assert!(!any_window_is_navigated(&[]));
    }

    #[test]
    fn a_written_record_reads_back_identical() {
        let tmp = tempfile::tempdir().unwrap();
        let session = capture(
            &WindowSession::default(),
            &[snapshot(
                "main",
                "http://localhost:3210/myws/",
                rect(1, 2, 1200, 800),
            )],
        );
        assert_eq!(session.units, FrameUnits::LogicalPoints);
        write(tmp.path(), &session);
        assert_eq!(read(tmp.path()), session);
    }

    #[test]
    fn a_legacy_record_keeps_its_open_set_and_drops_its_frames() {
        let tmp = tempfile::tempdir().unwrap();
        write_raw(
            tmp.path(),
            r#"{"open":["myws","dev"],
                "geometry":{"myws":{"x":4050,"y":3250,"width":3456,"height":2168}}}"#,
        );
        let session = read(tmp.path());
        assert_eq!(workspaces(&session), vec!["myws", "dev"]);
        assert!(session.geometry.is_empty());
        assert_eq!(session.units, FrameUnits::LogicalPoints);
    }

    #[test]
    fn a_stamped_record_keeps_its_frames() {
        let tmp = tempfile::tempdir().unwrap();
        write_raw(
            tmp.path(),
            r#"{"open":["myws"],"units":"logical-points",
                "geometry":{"myws":{"x":100,"y":200,"width":800,"height":600}}}"#,
        );
        assert_eq!(
            frame_of(&read(tmp.path()), "myws"),
            Some(held(rect(100, 200, 800, 600)))
        );
    }

    #[test]
    fn a_missing_or_unreadable_record_restores_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read(tmp.path()), WindowSession::default());

        for body in [r#"{ not json"#, "", r#"{"open":"myws"}"#, "[]"] {
            write_raw(tmp.path(), body);
            assert_eq!(
                read(tmp.path()),
                WindowSession::default(),
                "unreadable: {body:?}"
            );
        }
    }

    #[test]
    fn a_partial_record_keeps_the_half_it_carries() {
        let tmp = tempfile::tempdir().unwrap();
        write_raw(tmp.path(), r#"{"open":["myws"]}"#);
        let session = read(tmp.path());
        assert_eq!(workspaces(&session), vec!["myws"]);
        assert!(session.geometry.is_empty());
    }

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
        assert_eq!(
            frame_for_url(&session, "http://localhost:3210/dev/"),
            Some(held(rect(100, 50, 900, 700)))
        );
        assert_eq!(
            frame_for_url(&session, "http://localhost:3210/myws/"),
            Some(held(rect(0, 0, 1200, 800)))
        );
        assert_eq!(
            frame_for_url(&session, "http://localhost:3210/dev/#notifications"),
            Some(held(rect(100, 50, 900, 700)))
        );
    }

    #[test]
    fn a_url_with_no_recorded_frame_asks_for_the_default() {
        let session = WindowSession {
            open: unlabelled(&["myws"]),
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

    #[test]
    fn the_plan_pairs_each_workspace_with_its_frame() {
        let session = WindowSession {
            open: unlabelled(&["myws", "dev"]),
            geometry: BTreeMap::from([("myws".to_string(), held(rect(1, 2, 1200, 800)))]),
            units: FrameUnits::LogicalPoints,
        };
        assert_eq!(
            restore_plan(&session, true),
            vec![
                fresh("myws", Some(held(rect(1, 2, 1200, 800)))),
                fresh("dev", None),
            ]
        );
    }

    #[test]
    fn the_plan_drops_anything_that_is_not_a_workspace_slug() {
        let session = WindowSession {
            open: unlabelled(&["..", "a/b", "MyWs", "~", "", "http://evil.example", "ok"]),
            geometry: BTreeMap::new(),
            units: FrameUnits::LogicalPoints,
        };
        assert_eq!(restore_plan(&session, true), vec![fresh("ok", None)]);
    }

    #[test]
    fn a_launch_that_wants_no_window_restores_nothing() {
        let session = WindowSession {
            open: unlabelled(&["myws"]),
            geometry: BTreeMap::new(),
            units: FrameUnits::LogicalPoints,
        };
        assert!(restore_plan(&session, false).is_empty());
    }

    const WINDOW_A: &str = "window-0123456789abcdef0123456789abcdef";
    const WINDOW_B: &str = "window-fedcba9876543210fedcba9876543210";

    #[test]
    fn two_windows_on_one_workspace_come_back_as_two_under_their_labels() {
        let tmp = tempfile::tempdir().unwrap();
        let session = capture(
            &WindowSession::default(),
            &[
                snapshot(
                    WINDOW_A,
                    "http://localhost:3210/dev/",
                    rect(500, 0, 900, 700),
                ),
                snapshot("main", "http://localhost:3210/dev/", rect(0, 0, 1200, 800)),
            ],
        );
        write(tmp.path(), &session);
        let plan = restore_plan(&read(tmp.path()), true);
        assert_eq!(
            plan,
            vec![
                RestoredWindow {
                    label: Some("main".to_string()),
                    workspace: "dev".to_string(),
                    frame: Some(held(rect(0, 0, 1200, 800))),
                },
                RestoredWindow {
                    label: Some(WINDOW_A.to_string()),
                    workspace: "dev".to_string(),
                    frame: Some(held(rect(500, 0, 900, 700))),
                },
            ]
        );
    }

    #[test]
    fn a_rescued_window_keeps_its_own_held_frame() {
        let before = capture(
            &WindowSession::default(),
            &[
                snapshot("main", "http://localhost:3210/dev/", rect(0, 0, 1200, 800)),
                snapshot(
                    WINDOW_A,
                    "http://localhost:3210/dev/",
                    rect(900, -300, 900, 700),
                ),
            ],
        );
        let after = capture(
            &before,
            &[
                snapshot("main", "http://localhost:3210/dev/", rect(0, 0, 1200, 800)),
                rescued_snapshot(
                    WINDOW_A,
                    "http://localhost:3210/dev/",
                    rect(900, 30, 900, 700),
                ),
            ],
        );
        assert_eq!(after.open[1].frame, Some(held(rect(900, -300, 900, 700))));
    }

    #[test]
    fn a_forged_label_restores_under_a_fresh_one() {
        let session = WindowSession {
            open: ["window-1", "url-preview-3", "window-../x", "Main", "evil"]
                .iter()
                .map(|label| OpenWindow {
                    label: Some(label.to_string()),
                    workspace: "dev".to_string(),
                    frame: None,
                })
                .collect(),
            ..WindowSession::default()
        };
        let plan = restore_plan(&session, true);
        assert_eq!(plan.len(), 5);
        assert!(plan.iter().all(|window| window.label.is_none()));
    }

    #[test]
    fn a_repeated_label_restores_once_under_it() {
        let session = WindowSession {
            open: [WINDOW_A, WINDOW_A]
                .iter()
                .map(|label| OpenWindow {
                    label: Some(label.to_string()),
                    workspace: "dev".to_string(),
                    frame: None,
                })
                .collect(),
            ..WindowSession::default()
        };
        let labels: Vec<Option<String>> = restore_plan(&session, true)
            .into_iter()
            .map(|window| window.label)
            .collect();
        assert_eq!(labels, vec![Some(WINDOW_A.to_string()), None]);
    }

    #[test]
    fn the_window_recorded_as_main_restores_first() {
        let session = WindowSession {
            open: [(WINDOW_B, "b"), ("main", "a")]
                .iter()
                .map(|(label, id)| OpenWindow {
                    label: Some(label.to_string()),
                    workspace: id.to_string(),
                    frame: None,
                })
                .collect(),
            ..WindowSession::default()
        };
        let plan = restore_plan(&session, true);
        assert_eq!(plan[0].label.as_deref(), Some("main"));
        assert_eq!(plan[1].label.as_deref(), Some(WINDOW_B));
    }

    #[test]
    fn a_record_of_bare_slugs_restores_each_as_a_fresh_window() {
        let tmp = tempfile::tempdir().unwrap();
        write_raw(
            tmp.path(),
            r#"{"open":["myws","dev"],"units":"logical-points",
                "geometry":{"dev":{"x":1,"y":2,"width":900,"height":700}}}"#,
        );
        assert_eq!(
            restore_plan(&read(tmp.path()), true),
            vec![
                fresh("myws", None),
                fresh("dev", Some(held(rect(1, 2, 900, 700))))
            ]
        );
    }
}
