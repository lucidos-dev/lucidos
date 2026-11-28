use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use tauri::Manager;
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

use crate::{desktop, window_restore, window_session};

pub(crate) fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
}

pub(crate) fn plugin_tracks(label: &str) -> bool {
    label == crate::app_window::MAIN_WINDOW_LABEL
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowRecords {
    Packaged,
    Dev(PathBuf),
    Nowhere,
}

impl WindowRecords {
    fn resolve(is_dev: bool, dev_dir: Option<OsString>) -> Self {
        if !is_dev {
            return Self::Packaged;
        }
        match dev_dir.map(PathBuf::from) {
            Some(dir) if dir.is_absolute() && dir.to_str().is_some() => Self::Dev(dir),
            _ => Self::Nowhere,
        }
    }

    pub(crate) fn keeps_records(&self) -> bool {
        !matches!(self, Self::Nowhere)
    }

    pub(crate) fn session_dir(&self) -> Option<PathBuf> {
        match self {
            Self::Packaged => desktop::app_data_dir_from_env().ok(),
            Self::Dev(dir) => Some(dir.clone()),
            Self::Nowhere => None,
        }
    }

    fn plugin_filename(&self) -> Option<String> {
        match self {
            Self::Dev(dir) => dir
                .join(tauri_plugin_window_state::DEFAULT_FILENAME)
                .to_str()
                .map(str::to_owned),
            Self::Packaged | Self::Nowhere => None,
        }
    }
}

pub(crate) fn window_records() -> &'static WindowRecords {
    static RECORDS: std::sync::OnceLock<WindowRecords> = std::sync::OnceLock::new();
    RECORDS.get_or_init(|| {
        WindowRecords::resolve(
            tauri::is_dev(),
            std::env::var_os(crate::dev_window::RECORDS_DIR_ENV),
        )
    })
}

pub(crate) fn window_state_plugin<R: tauri::Runtime>() -> Option<tauri::plugin::TauriPlugin<R>> {
    let records = window_records();
    if !records.keeps_records() {
        return None;
    }
    let builder = tauri_plugin_window_state::Builder::new()
        .with_state_flags(window_state_flags())
        .with_filter(plugin_tracks);
    let builder = match records.plugin_filename() {
        Some(filename) => builder.with_filename(filename),
        None => builder,
    };
    Some(builder.build())
}

const WINDOW_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(600);

const WINDOW_POLL: std::time::Duration = std::time::Duration::from_millis(300);

pub(crate) struct WindowSaver {
    dirty: AtomicBool,
    last_change: Mutex<Instant>,
}

impl Default for WindowSaver {
    fn default() -> Self {
        Self {
            dirty: AtomicBool::new(false),
            last_change: Mutex::new(Instant::now()),
        }
    }
}

impl WindowSaver {
    fn note_changed(&self) {
        *self.last_change.lock().unwrap() = Instant::now();
        self.dirty.store(true, Ordering::Release);
    }

    fn write_is_due(&self) -> bool {
        should_persist_windows(
            self.dirty.load(Ordering::Acquire),
            self.last_change.lock().unwrap().elapsed(),
        )
    }

    fn take_write(&self) {
        self.dirty.store(false, Ordering::Release);
    }

    #[cfg(test)]
    fn owes_a_write(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }
}

fn should_persist_windows(dirty: bool, since_last_change: std::time::Duration) -> bool {
    dirty && since_last_change >= WINDOW_SAVE_DEBOUNCE
}

pub(crate) fn note_windows_changed(app: &tauri::AppHandle) {
    app.state::<WindowSaver>().note_changed();
}

pub(crate) fn spawn_window_flush(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(WINDOW_POLL);
        let saver = app.state::<WindowSaver>();
        if saver.write_is_due() {
            saver.take_write();
            persist_window_state_on_main(&app);
        }
    });
}

fn persist_window_state_on_main(app: &tauri::AppHandle) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || persist_windows(&handle)) {
        eprintln!("[Tauri] Failed to schedule window-state save: {e}");
    }
}

pub(crate) fn persist_windows(app: &tauri::AppHandle) {
    save_plugin_state(app, "Failed to persist window state");
    persist_window_session(app);
}

fn save_plugin_state(app: &tauri::AppHandle, what: &str) {
    if !window_records().keeps_records() || main_is_wearing_a_rescue(app) {
        return;
    }
    if let Err(e) = app.save_window_state(window_state_flags()) {
        eprintln!("[Tauri] {what}: {e}");
    }
}

fn main_is_wearing_a_rescue(app: &tauri::AppHandle) -> bool {
    let Some(window) = app.get_window(crate::app_window::MAIN_WINDOW_LABEL) else {
        return false;
    };
    let Some(frame) = window_restore::live_frame(&window) else {
        return false;
    };
    window_restore::is_wearing_a_rescue(crate::app_window::MAIN_WINDOW_LABEL, frame)
}

pub(crate) fn persist_window_state_only(app: &tauri::AppHandle) {
    save_plugin_state(app, "Failed to persist window state on close");
}

pub(crate) fn resolve_window_session_plan() -> &'static [window_session::RestoredWindow] {
    static PLAN: std::sync::OnceLock<Vec<window_session::RestoredWindow>> =
        std::sync::OnceLock::new();
    PLAN.get_or_init(|| {
        let Some(dir) = window_records().session_dir() else {
            return Vec::new();
        };
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let restore = crate::should_show_window_at_startup(&args, false);
        window_session::restore_plan(&window_session::read(&dir), restore)
    })
}

pub(crate) fn readable_window_session() -> window_session::WindowSession {
    window_records()
        .session_dir()
        .map(|dir| window_session::read(&dir))
        .unwrap_or_default()
}

pub(crate) fn remembered_frame(url: &str) -> Option<window_restore::RememberedFrame> {
    window_session::frame_for_url(&readable_window_session(), url)
}

pub(crate) fn size_main_window_for_its_workspace(
    app: &tauri::AppHandle,
    frame: window_restore::Rect,
) {
    let Some(window) = app.get_window(crate::app_window::MAIN_WINDOW_LABEL) else {
        return;
    };
    if window.is_fullscreen().unwrap_or(false) {
        return;
    }
    crate::app_window::place_window(&window, frame, "the main window for its workspace");
}

static TEARING_DOWN: AtomicBool = AtomicBool::new(false);

pub(crate) fn begin_teardown() {
    TEARING_DOWN.store(true, Ordering::SeqCst);
}

pub(crate) fn tearing_down() -> bool {
    TEARING_DOWN.load(Ordering::SeqCst)
}

struct PresentedGate(AtomicBool);

impl PresentedGate {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn note_presented(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn may_write(&self, any_window_is_navigated: bool) -> bool {
        self.0.load(Ordering::SeqCst) && any_window_is_navigated
    }

    fn capture(
        &self,
        previous: &window_session::WindowSession,
        windows: &[window_session::WindowSnapshot],
        is_dismissed: impl Fn(&str) -> bool,
    ) -> Option<window_session::WindowSession> {
        if !self.may_write(window_session::any_window_is_navigated(windows)) {
            return None;
        }
        let mut session = window_session::capture(previous, windows);
        session
            .open
            .retain(|window| !window.label.as_deref().is_some_and(&is_dismissed));
        Some(session)
    }
}

static PRESENTED: PresentedGate = PresentedGate::new();

pub(crate) fn note_presented() {
    PRESENTED.note_presented();
}

pub(crate) fn forget_closed_window(app: &tauri::AppHandle) {
    if tearing_down() {
        return;
    }
    persist_window_session(app);
}

pub(crate) fn persist_window_session(app: &tauri::AppHandle) {
    let Some(dir) = window_records().session_dir() else {
        return;
    };
    let windows: Vec<window_session::WindowSnapshot> = app
        .webviews()
        .into_iter()
        .filter(|(label, _)| crate::app_window::is_app_window(label))
        .filter_map(|(label, webview)| {
            let window = webview.window();
            let (Ok(url), Some(frame)) = (webview.url(), window_restore::live_frame(&window))
            else {
                return None;
            };
            Some(window_session::WindowSnapshot {
                rescued: window_restore::is_wearing_a_rescue(&label, frame),
                whereabouts: window_restore::whereabouts_now(app, &label, frame),
                label,
                url: url.to_string(),
                frame,
            })
        })
        .collect();
    let previous = window_session::read(&dir);
    if let Some(session) =
        PRESENTED.capture(&previous, &windows, crate::window_screen::is_dismissed)
    {
        window_session::write(&dir, &session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_launch_that_never_showed_a_window_writes_nothing() {
        let gate = PresentedGate::new();
        assert!(!gate.may_write(true), "navigated is not enough on its own");
        assert!(!gate.may_write(false));
    }

    #[test]
    fn a_launch_still_on_the_splash_writes_nothing() {
        let gate = PresentedGate::new();
        gate.note_presented();
        assert!(
            !gate.may_write(false),
            "a shown window is not enough either"
        );
    }

    #[test]
    fn a_shown_and_navigated_launch_writes() {
        let gate = PresentedGate::new();
        gate.note_presented();
        assert!(gate.may_write(true));
    }

    #[test]
    fn the_gate_stays_open_once_a_window_has_been_shown() {
        let gate = PresentedGate::new();
        gate.note_presented();
        gate.note_presented();
        assert!(gate.may_write(true));
    }

    #[test]
    fn dismissing_the_last_window_writes_an_empty_arrangement_with_its_frame() {
        let gate = PresentedGate::new();
        gate.note_presented();
        let windows = [window_session::WindowSnapshot {
            label: "main".into(),
            url: "https://example.com/dev/".into(),
            frame: window_restore::Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            whereabouts: window_restore::Whereabouts::OnScreen(None),
            rescued: false,
        }];
        let previous = gate
            .capture(&Default::default(), &windows, |_| false)
            .unwrap();
        assert_eq!(previous.open.len(), 1);
        let closed = gate
            .capture(&previous, &windows, |label| label == "main")
            .unwrap();
        assert!(closed.open.is_empty());
        assert_eq!(closed.geometry, previous.geometry);
    }

    #[test]
    fn a_write_refused_on_the_splash_is_armed_again_by_the_navigation() {
        let saver = WindowSaver::default();
        saver.note_changed();
        assert!(saver.owes_a_write(), "the startup placement owes a write");

        saver.take_write();
        assert!(!saver.owes_a_write(), "the flush is a one-shot");

        assert!(!window_session::any_window_is_navigated(&[]));

        saver.note_changed();
        assert!(
            saver.owes_a_write(),
            "the navigation owes the record a write the flush already spent"
        );
    }

    #[test]
    fn should_persist_windows_waits_for_quiet_then_fires() {
        assert!(!should_persist_windows(false, Duration::from_secs(10)));
        assert!(!should_persist_windows(true, Duration::from_millis(0)));
        assert!(!should_persist_windows(
            true,
            WINDOW_SAVE_DEBOUNCE - Duration::from_millis(1)
        ));
        assert!(should_persist_windows(true, WINDOW_SAVE_DEBOUNCE));
        assert!(should_persist_windows(
            true,
            WINDOW_SAVE_DEBOUNCE + Duration::from_millis(1)
        ));
    }

    #[test]
    fn the_plugin_tracks_main_and_nothing_else() {
        assert!(plugin_tracks(crate::app_window::MAIN_WINDOW_LABEL));
        for label in ["window-0", "window-7", "url-preview-1", "lucidos-tray", ""] {
            assert!(!plugin_tracks(label), "{label:?} is tracked");
        }
    }

    #[test]
    fn the_packaged_client_keeps_its_records_where_it_always_has() {
        for dev_dir in [None, Some("/checkout/.launch/dev-window".into())] {
            let records = WindowRecords::resolve(false, dev_dir);
            assert_eq!(records, WindowRecords::Packaged);
            assert_eq!(records.plugin_filename(), None);
            assert!(records.keeps_records());
            assert_eq!(records.session_dir(), desktop::app_data_dir_from_env().ok());
        }
    }

    #[test]
    fn a_dev_window_never_records_in_the_packaged_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let records = WindowRecords::resolve(true, Some(dir.path().into()));
        assert_eq!(records, WindowRecords::Dev(dir.path().to_path_buf()));
        assert_eq!(records.session_dir().as_deref(), Some(dir.path()));
        assert!(records.keeps_records());
        let packaged_config = std::path::Path::new("/Users/me/Library/Application Support/x");
        let plugin_file = packaged_config.join(records.plugin_filename().unwrap());
        assert_eq!(
            plugin_file,
            dir.path().join(tauri_plugin_window_state::DEFAULT_FILENAME)
        );
    }

    #[test]
    fn a_dev_binary_with_no_dir_of_its_own_records_nothing() {
        for dev_dir in [None, Some("".into()), Some("relative/dev-window".into())] {
            let records = WindowRecords::resolve(true, dev_dir.clone());
            assert_eq!(records, WindowRecords::Nowhere, "{dev_dir:?}");
            assert_eq!(records.session_dir(), None);
            assert_eq!(records.plugin_filename(), None);
            assert!(!records.keeps_records());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_dev_dir_that_is_not_utf8_counts_as_none() {
        use std::os::unix::ffi::OsStringExt;
        let dir = OsString::from_vec(b"/checkout/\xff/dev-window".to_vec());
        assert_eq!(
            WindowRecords::resolve(true, Some(dir)),
            WindowRecords::Nowhere
        );
    }

    #[test]
    fn window_state_flags_remembers_geometry_not_visibility() {
        let flags = window_state_flags();
        assert!(flags.contains(StateFlags::SIZE));
        assert!(flags.contains(StateFlags::POSITION));
        assert!(flags.contains(StateFlags::MAXIMIZED));
        assert!(flags.contains(StateFlags::FULLSCREEN));
        assert!(!flags.contains(StateFlags::VISIBLE));
        assert!(!flags.contains(StateFlags::DECORATIONS));
    }
}
