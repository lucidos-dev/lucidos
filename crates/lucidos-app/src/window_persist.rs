//! Remembering the user's windows across a launch, and the gates on writing.
//!
//! Two records, and they do not overlap. The window-state plugin covers `main`
//! ALONE, which is the one label that means the same thing every launch. It
//! restores that window before anything knows what it will show, and it is
//! where maximized and fullscreen live. The session record is per WORKSPACE:
//! which ones had a window, and how big each was. Every other window is its
//! business, and [`plugin_tracks`] is what holds the split.
//!
//! Both are written together, so a window recorded in one is recorded in the
//! other. Two gates decide whether a write happens at all: a teardown is not
//! the user closing their windows, and a launch nobody looked at has no
//! arrangement to record.
//!
//! The clamp that sanitises a restored frame is `window_restore`, and the
//! record's own format is `window_session`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use tauri::Manager;
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

use crate::{desktop, window_restore, window_session};

/// Window state the app persists and restores via `tauri-plugin-window-state`.
/// Deliberately EXCLUDES two flags:
/// - `VISIBLE`: the packaged client hides its window rather than closing it. A
///   flush taken while hidden would persist `visible: false`, and the plugin
///   would restore the window hidden on the next launch.
/// - `DECORATIONS`: toggling it on macOS rebuilds the NSWindow style mask and
///   can drop the `titleBarStyle: "Overlay"` configuration, turning the
///   reclaimed title-bar band back into an opaque bar.
pub(crate) fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
}

/// Which windows `tauri-plugin-window-state` may restore and save: `main`, and
/// nothing else. Passed to its builder as the tracking filter.
///
/// The plugin keys on the window LABEL, and `window-<n>` comes off a counter
/// that resets each process. So that key names a different window every launch,
/// which is the whole reason ADR 0123 keys the session by workspace instead.
/// Unfiltered, the plugin restored one runtime window's geometry onto an
/// unrelated one, and `MAXIMIZED` and `FULLSCREEN` with it. Placing a frame
/// undoes neither, so a fresh window could come up fullscreen inherited from a
/// past life.
///
/// This is the "for `main` only" the ADR already promised. `main` is declared in
/// `tauri.conf.json`, so its label does mean one window across launches.
pub(crate) fn plugin_tracks(label: &str) -> bool {
    label == crate::app_window::MAIN_WINDOW_LABEL
}

/// How long the window must sit still (no move/resize) before the debounced
/// background flush writes `.window-state.json`. Short enough that a quick
/// move-then-relaunch is remembered, long enough that a drag doesn't thrash the
/// disk on every intermediate `Moved`/`Resized` event.
const GEOMETRY_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(600);

/// How often the flush thread wakes to ask whether the window has gone quiet.
const GEOMETRY_POLL: std::time::Duration = std::time::Duration::from_millis(300);

/// Coordinates the debounced geometry flush. The window-state plugin writes to
/// disk only on `RunEvent::Exit`, which the packaged client never reaches.
/// Without this, a moved or resized window is remembered in memory alone. A
/// background thread flushes once the window has been quiet for
/// [`GEOMETRY_SAVE_DEBOUNCE`] (see [`should_persist_geometry`]).
pub(crate) struct GeometrySaver {
    dirty: AtomicBool,
    last_change: Mutex<Instant>,
}

impl Default for GeometrySaver {
    fn default() -> Self {
        Self {
            dirty: AtomicBool::new(false),
            last_change: Mutex::new(Instant::now()),
        }
    }
}

/// Whether the debounced flush should run now: there is unsaved geometry and
/// the window has been quiet at least [`GEOMETRY_SAVE_DEBOUNCE`].
fn should_persist_geometry(dirty: bool, since_last_change: std::time::Duration) -> bool {
    dirty && since_last_change >= GEOMETRY_SAVE_DEBOUNCE
}

/// A window moved or resized, so the flush is owed. Called from the window
/// event handler on every intermediate event, which is why the write itself is
/// debounced rather than immediate.
pub(crate) fn note_geometry_changed(app: &tauri::AppHandle) {
    let saver = app.state::<GeometrySaver>();
    *saver.last_change.lock().unwrap() = Instant::now();
    saver.dirty.store(true, Ordering::Release);
}

/// Start the debounced geometry flush. Its own thread, for the life of the
/// process. The save is marshalled onto the main thread, for the reason
/// [`persist_window_state_on_main`] gives.
pub(crate) fn spawn_geometry_flush(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(GEOMETRY_POLL);
        let saver = app.state::<GeometrySaver>();
        let dirty = saver.dirty.load(Ordering::Acquire);
        let since = saver.last_change.lock().unwrap().elapsed();
        if should_persist_geometry(dirty, since) {
            saver.dirty.store(false, Ordering::Release);
            persist_window_state_on_main(&app);
        }
    });
}

/// Persist window geometry, forcing the work onto the MAIN thread.
///
/// `tauri-plugin-window-state::save_window_state` holds an internal cache lock
/// while it reads each window's live geometry. Off the main thread those getters
/// block on a round-trip to the event loop. So a worker-thread save holds the
/// cache lock across a wait for the main thread, while the main thread blocks
/// taking that same lock: a full-UI deadlock. Every caller NOT already on the
/// main thread must route through here. Fire-and-forget, so the save runs on the
/// next main-loop turn.
fn persist_window_state_on_main(app: &tauri::AppHandle) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || persist_windows(&handle)) {
        eprintln!("[Tauri] Failed to schedule window-state save: {e}");
    }
}

/// Persist BOTH records the client keeps about its windows. Main thread only,
/// for the reason [`persist_window_state_on_main`] gives.
///
/// Every save site calls this, so a window recorded in one file is recorded in
/// the other. The module header says why there are two.
pub(crate) fn persist_windows(app: &tauri::AppHandle) {
    if let Err(e) = app.save_window_state(window_state_flags()) {
        eprintln!("[Tauri] Failed to persist window state: {e}");
    }
    persist_window_session(app);
}

/// Persist the plugin's record alone, without the session.
///
/// The one caller is a window CLOSE, where the session is written separately
/// and only when the close is the user's own. See the `CloseRequested` arm.
pub(crate) fn persist_window_state_only(app: &tauri::AppHandle) {
    if let Err(e) = app.save_window_state(window_state_flags()) {
        eprintln!("[Tauri] Failed to persist window state on close: {e}");
    }
}

/// What this launch restores: a workspace per window, and the frame each wants.
///
/// Resolved once, in `setup`, and read again by `desktop::launch` on its own
/// thread once the gateway is healthy. Memoized because the two reads must
/// agree: the first window to settle rewrites the record underneath them.
///
/// A workspace with no remembered frame still gets a window, built at the
/// default size.
pub(crate) fn resolve_window_session_plan() -> &'static [(String, Option<window_restore::Rect>)] {
    static PLAN: std::sync::OnceLock<Vec<(String, Option<window_restore::Rect>)>> =
        std::sync::OnceLock::new();
    PLAN.get_or_init(|| {
        // Dev restores nothing: it shares the packaged app-data dir and
        // `desktop::launch` is a no-op there, so the record is not its to read.
        if tauri::is_dev() {
            return Vec::new();
        }
        let Ok(app_data) = desktop::app_data_dir_from_env() else {
            return Vec::new();
        };
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let restore = crate::should_show_window_at_startup(&args, false);
        window_session::restore_plan(&window_session::read(&app_data), restore)
    })
}

/// The window session this process may act on, or an empty one when there is
/// none to read.
///
/// Dev is empty by construction. It shares the packaged app-data dir, so acting
/// on the record would let a dev run rearrange the packaged client's windows.
/// The same rule [`persist_window_session`] follows on the write side.
pub(crate) fn readable_window_session() -> window_session::WindowSession {
    if tauri::is_dev() {
        return window_session::WindowSession::default();
    }
    desktop::app_data_dir_from_env()
        .map(|app_data| window_session::read(&app_data))
        .unwrap_or_default()
}

/// The frame the workspace `url` serves was last left at, for a window about to
/// be built for it. `None` asks for the declared default.
///
/// Reads the record fresh rather than the memoized launch plan. The two answer
/// different questions: that one is what THIS launch owed, and this is where a
/// workspace was left, which the user has been rearranging ever since.
///
/// A file read per call is right here. Every caller is a click or a banner tap,
/// and a stale answer would place the window wrong.
pub(crate) fn remembered_frame(url: &str) -> Option<window_restore::Rect> {
    window_session::frame_for_url(&readable_window_session(), url)
}

/// Put `main` at the frame the workspace it will open remembers.
///
/// Before the show, so the window appears at its size rather than jumping to it
/// a second later. The clamp runs after and sanitises whatever this leaves.
///
/// This is the one placement that can meet an ALREADY arranged window. It runs
/// from `setup` after the plugin restored `main`, and from a reopen pointing an
/// adrift `main` at a workspace. So it is the one that has to skip a fullscreen
/// window, for the reason `window_restore::clamp_restored_geometry` gives: macOS
/// owns that frame, and sizing it fights the AppKit transition. A window built
/// for a frame cannot be fullscreen yet, so no other caller needs the test.
///
/// By window, not webview window, per ADR 0140. Sizing and placing are window
/// operations, and by the time a reopen runs `main` is the likeliest window of
/// all to be hosting a URL preview.
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

/// Set once the client has begun a deliberate teardown: a quit, a restart, or
/// the relaunch an update ends with.
///
/// Every one of those destroys its windows on the way out, one at a time. The
/// `Destroyed` recapture would read that as the user closing them, and empty
/// the record milliseconds after the teardown wrote it. The last record written
/// BEFORE the teardown is the one that must stand.
static TEARING_DOWN: AtomicBool = AtomicBool::new(false);

/// Declare that the windows are about to go away because the client is. Called
/// by each deliberate exit path, right after its own [`persist_windows`].
pub(crate) fn begin_teardown() {
    TEARING_DOWN.store(true, Ordering::SeqCst);
}

/// Is the client on its way out? A close arriving now is the teardown's, not
/// the user's.
pub(crate) fn tearing_down() -> bool {
    TEARING_DOWN.load(Ordering::SeqCst)
}

/// Whether this launch has ever put a window on screen, and the gate that rides
/// on it.
///
/// The half of the session-write gate that rules out a LOGIN START. That launch
/// comes up menu-bar-only and shows nothing, while `desktop::launch` still
/// navigates the hidden `main` to the gateway root. The navigation half of the
/// gate therefore passes, and the write replaced the user's arrangement with
/// the empty one nobody was looking at.
///
/// A latch rather than a live visibility test. A hidden window IS part of the
/// arrangement, since `main` is hidden rather than closed and the tray brings
/// it back on its workspace. Testing visibility instead blocked the one write
/// whose job is to SHRINK the record. What must not count is a launch where the
/// user never saw anything at all.
///
/// A struct rather than a bare static, for the reason `StartupShow` is one:
/// the rule is then testable against an instance a test owns.
struct PresentedGate(AtomicBool);

impl PresentedGate {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// A window is now on screen. Every path that puts one there says so.
    fn note_presented(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Is the session record worth writing? BOTH halves, so the one expression
    /// carries the whole rule.
    fn may_write(&self, any_window_is_navigated: bool) -> bool {
        self.0.load(Ordering::SeqCst) && any_window_is_navigated
    }
}

static PRESENTED: PresentedGate = PresentedGate::new();

/// A window just reached the screen. Every path that shows one says so, which
/// is what opens the session-write gate. See [`PresentedGate`].
pub(crate) fn note_presented() {
    PRESENTED.note_presented();
}

/// Re-record the window set after one closed, so the record stops naming it.
///
/// Stood down by a teardown, since a window destroyed on the way out was not
/// closed by the user. See [`TEARING_DOWN`].
pub(crate) fn forget_closed_window(app: &tauri::AppHandle) {
    if tearing_down() {
        return;
    }
    persist_window_session(app);
}

/// Fold the live windows into the session record and write it.
///
/// Reading each window's geometry is a main-thread call, same as the plugin's
/// save, so this is only ever reached through [`persist_windows`].
pub(crate) fn persist_window_session(app: &tauri::AppHandle) {
    // Dev shares the packaged install's app-data dir, and `desktop::launch`
    // restores nothing there. Writing would only let a dev run rearrange the
    // packaged client's windows.
    if tauri::is_dev() {
        return;
    }
    let Ok(app_data) = desktop::app_data_dir_from_env() else {
        // No `HOME`, so nowhere to keep a record.
        return;
    };
    // Enumerates WEBVIEWS and reaches each window through `webview.window()`,
    // per ADR 0140. A snapshot needs the URL, a page read, and the geometry, a
    // window read. The blind flavour dropped whichever window was hosting a URL
    // preview, and the next launch forgot its workspace and frame (ADR 0123).
    let windows: Vec<window_session::WindowSnapshot> = app
        .webviews()
        .into_iter()
        .filter(|(label, _)| crate::app_window::is_app_window(label))
        .filter_map(|(label, webview)| {
            let window = webview.window();
            // The same pair the plugin persists and `window_restore` clamps, so
            // all three reason about one set of numbers.
            let (Ok(url), Ok(position), Ok(size)) =
                (webview.url(), window.outer_position(), window.inner_size())
            else {
                return None;
            };
            Some(window_session::WindowSnapshot {
                label,
                url: url.to_string(),
                frame: window_restore::Rect {
                    x: position.x as i64,
                    y: position.y as i64,
                    width: size.width as i64,
                    height: size.height as i64,
                },
            })
        })
        .collect();
    // A launch that never showed a window has no arrangement to record, and
    // neither has one whose windows are all still on the splash. Each emptied
    // the record through a different writer before this gate existed.
    if !PRESENTED.may_write(window_session::any_window_is_navigated(&windows)) {
        return;
    }
    let previous = window_session::read(&app_data);
    window_session::write(&app_data, &window_session::capture(&previous, &windows));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // A login start comes up menu-bar-only, and `desktop::launch` navigates the
    // hidden `main` to the gateway root anyway. The navigation half therefore
    // passes on its own, and writing then replaced the whole record with an
    // empty one nobody was looking at.
    #[test]
    fn a_launch_that_never_showed_a_window_writes_nothing() {
        let gate = PresentedGate::new();
        assert!(!gate.may_write(true), "navigated is not enough on its own");
        assert!(!gate.may_write(false));
    }

    // Boot: the startup geometry write arms the debounced flush while every
    // window still sits on the splash.
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

    // A latch, not a live visibility test. `main` is hidden rather than closed,
    // so a trayed client must still be able to record a window closing.
    #[test]
    fn the_gate_stays_open_once_a_window_has_been_shown() {
        let gate = PresentedGate::new();
        gate.note_presented();
        gate.note_presented();
        assert!(gate.may_write(true));
    }

    #[test]
    fn should_persist_geometry_waits_for_quiet_then_fires() {
        // Nothing changed, so never flush, however long it has been.
        assert!(!should_persist_geometry(false, Duration::from_secs(10)));
        // Dirty, but the user is still moving or resizing, so wait.
        assert!(!should_persist_geometry(true, Duration::from_millis(0)));
        assert!(!should_persist_geometry(
            true,
            GEOMETRY_SAVE_DEBOUNCE - Duration::from_millis(1)
        ));
        // Dirty and quiet for at least the debounce window, so flush.
        assert!(should_persist_geometry(true, GEOMETRY_SAVE_DEBOUNCE));
        assert!(should_persist_geometry(
            true,
            GEOMETRY_SAVE_DEBOUNCE + Duration::from_millis(1)
        ));
    }

    // The plugin keys on the label, and only `main`'s means one window across
    // launches. Tracking a `window-<n>` restored a past session's second window
    // onto whatever this session's second window turned out to be, fullscreen
    // flag included.
    #[test]
    fn the_plugin_tracks_main_and_nothing_else() {
        assert!(plugin_tracks(crate::app_window::MAIN_WINDOW_LABEL));
        for label in ["window-0", "window-7", "url-preview-1", "lucidos-tray", ""] {
            assert!(!plugin_tracks(label), "{label:?} is tracked");
        }
    }

    #[test]
    fn window_state_flags_remembers_geometry_not_visibility() {
        let flags = window_state_flags();
        // Remember where the window is, how big, and on which screen.
        assert!(flags.contains(StateFlags::SIZE));
        assert!(flags.contains(StateFlags::POSITION));
        assert!(flags.contains(StateFlags::MAXIMIZED));
        assert!(flags.contains(StateFlags::FULLSCREEN));
        // But NOT VISIBLE (a flush taken while hidden would restore hidden) and
        // NOT DECORATIONS (toggling it on macOS drops the Overlay title bar).
        assert!(!flags.contains(StateFlags::VISIBLE));
        assert!(!flags.contains(StateFlags::DECORATIONS));
    }
}
