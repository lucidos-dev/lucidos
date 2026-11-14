//! The desk changed, so every window is asked whether it still fits.
//!
//! A window sized on a big display and carried to a small one keeps its size.
//! Every call site of the restore clamp is a window creation or a show, so
//! nothing used to notice. Undock, and the app hangs off the edge of the panel
//! that is left, with content nobody can reach.
//!
//! **The trigger is the DESK, never a window event.**
//! `NSApplicationDidChangeScreenParametersNotification` is the one signal every
//! variant shares: unplug, clamshell, a resolution change, a rearrangement. tao
//! emits `ScaleFactorChanged` only when the backing factor changes, and a
//! resolution change under a still window emits nothing at all.
//!
//! Two hazards fall out of that rather than being tuned away. A live drag posts
//! no screen-parameters change, and neither does our own correction. So the
//! pass cannot fire mid-drag, and cannot cause itself.
//!
//! It WAITS because AppKit posts more than once per reconfiguration, and can
//! post before it has relocated the windows. ADR 0204 has the rest, and
//! `window_restore::fit_to_displays` owns what the pass decides.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::Manager;

/// How long the desk must sit still before the pass runs. Long enough to
/// swallow one reconfiguration's burst, short enough that nobody sits looking
/// at a window hanging off the edge.
const DESK_SETTLE: Duration = Duration::from_millis(500);

/// How often the pass thread wakes to ask whether the desk has gone quiet.
const DESK_POLL: Duration = Duration::from_millis(250);

/// When the owed pass becomes due, or `None` when none is owed.
///
/// One value rather than a flag and a timestamp: "nothing pending" and "pending
/// until this moment" are the whole state, and a pair could hold neither.
static PASS_DUE_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// The desk changed, so a pass is owed and the quiet period starts again.
fn note_desk_changed() {
    *PASS_DUE_AT.lock().unwrap() = Some(Instant::now() + DESK_SETTLE);
}

/// Claim the owed pass, if the desk has been quiet long enough for it.
///
/// Pure, so the coalescing rule is testable without a clock. A claim is a
/// one-shot: nothing re-takes it until the desk changes again.
fn claim_due_pass(owed: &mut Option<Instant>, now: Instant) -> bool {
    match *owed {
        Some(due) if now >= due => {
            *owed = None;
            true
        }
        _ => false,
    }
}

/// Bring every app window back onto a display that can hold it.
///
/// Main thread only. Every getter behind `clamp_live_geometry` is a round trip
/// to the event loop, and the placement has to be issued from there.
///
/// By window, not webview window, per ADR 0140. Reading and correcting geometry
/// are window operations, and by now any window may be hosting a URL preview.
fn fit_windows_to_the_desk(app: &tauri::AppHandle) {
    for label in app.windows().into_keys() {
        if crate::app_window::is_app_window(&label) {
            crate::window_restore::clamp_live_geometry(app, &label);
        }
    }
}

/// Watch the desk, and correct a window it can no longer hold. Called once,
/// from `setup`, on the main thread.
///
/// This spawns the thread only when the observer went in, so a platform with no
/// desk notification to watch polls nothing.
///
/// Its own thread, the shape `window_persist::spawn_window_flush` already uses.
/// The two wait out different debts and share no state: that one holds a write
/// owed since the last change, this one a deadline each change pushes out.
pub(crate) fn watch_desk(app: tauri::AppHandle) {
    if !watch_screen_parameters() {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(DESK_POLL);
        // The lock is taken and dropped on this line, so nothing below holds it
        // across the hop to the main thread. That thread is where the observer
        // takes the same lock, and `window_persist::persist_window_state_on_main`
        // records what a wait held across it costs.
        let due = claim_due_pass(&mut PASS_DUE_AT.lock().unwrap(), Instant::now());
        if !due {
            continue;
        }
        let handle = app.clone();
        if let Err(e) = app.run_on_main_thread(move || fit_windows_to_the_desk(&handle)) {
            eprintln!("[Tauri] Could not schedule the desk fit pass: {e}");
        }
    });
}

/// Off macOS there is no screen-parameters notification to observe, so nothing
/// is watched and nothing polls.
#[cfg(not(target_os = "macos"))]
fn watch_screen_parameters() -> bool {
    false
}

/// Register for AppKit's display-reconfiguration notification. Returns whether
/// the registration went in.
#[cfg(target_os = "macos")]
fn watch_screen_parameters() -> bool {
    use objc2_foundation::{NSNotification, NSNotificationCenter};

    if objc2::MainThreadMarker::new().is_none() {
        eprintln!("[Tauri] Not on the main thread: the desk observer is not installed");
        return false;
    }
    let block = block2::RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
        note_desk_changed();
    });
    // SAFETY: reading AppKit's own notification-name constant. A nil object
    // takes every sender, and only the application posts this one. A nil queue
    // runs the block on the thread that posted, which is the main one. The
    // block only touches a mutex, so where it runs costs nothing.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSApplicationDidChangeScreenParametersNotification),
            None,
            None,
            &block,
        )
    };
    // Deliberately leaked: the registration must outlive every window, and no
    // teardown path could remove it at the right moment.
    std::mem::forget(observer);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One reconfiguration is one pass. AppKit posts the notification several
    /// times as the arrangement settles, and each post pushes the deadline out
    /// rather than queueing a pass of its own.
    #[test]
    fn a_burst_of_desk_changes_runs_one_pass() {
        let start = Instant::now();
        let mut owed = None;
        for step in 0..4 {
            let posted_at = start + Duration::from_millis(100 * step);
            owed = Some(posted_at + DESK_SETTLE);
            assert!(
                !claim_due_pass(&mut owed, posted_at),
                "a pass ran while the desk was still changing"
            );
        }
        // Quiet since the last post, so the one pass is due.
        assert!(claim_due_pass(&mut owed, start + DESK_SETTLE * 2));
    }

    /// A claim is a one-shot. Left owed, the poll thread would re-run the pass
    /// every tick for the life of the process.
    #[test]
    fn a_claimed_pass_is_not_re_taken() {
        let start = Instant::now();
        let mut owed = Some(start);
        assert!(claim_due_pass(&mut owed, start));
        assert_eq!(owed, None);
        assert!(!claim_due_pass(&mut owed, start + DESK_SETTLE * 10));
    }

    /// Nothing is owed until the desk changes, so an idle client never runs the
    /// pass however long it sits there.
    #[test]
    fn an_unchanged_desk_owes_no_pass() {
        let mut owed = None;
        assert!(!claim_due_pass(&mut owed, Instant::now()));
    }

    /// The deadline is in the future, so a tick inside the quiet period waits.
    #[test]
    fn a_pass_waits_for_the_quiet_period() {
        let start = Instant::now();
        let mut owed = Some(start + DESK_SETTLE);
        assert!(!claim_due_pass(&mut owed, start));
        assert!(!claim_due_pass(&mut owed, start + DESK_SETTLE - DESK_POLL));
        assert!(claim_due_pass(&mut owed, start + DESK_SETTLE));
    }
}
