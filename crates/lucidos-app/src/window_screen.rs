//! Whether the client believes one of its windows is on screen.
//!
//! A fact the client OWNS rather than infers. Every show records it here. Every
//! hide clears it. So the answer is our own intent, not a question put to
//! AppKit.
//!
//! That distinction is the point. A frozen window was reported with WebKit's own
//! log showing it suspending the page, because `[NSWindow isVisible]` answered
//! NO for a window the user was clicking. A watchdog that asks AppKit the same
//! question gets the same wrong answer, and stands down exactly when it is
//! needed. ADR 0180 records the decision and what the probe found.
//!
//! **Not the notification-presence question, and never to be folded into it.**
//! `native-window-active` asks whether the user is LOOKING at this window, so a
//! visible but unfocused one is deliberately not active. This asks whether the
//! window is on screen at all.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Labels the client has shown and has not hidden since.
static SHOWN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Whether the whole app is hidden, which Cmd-H does behind our back.
static APP_HIDDEN: AtomicBool = AtomicBool::new(false);

/// The client just put `label` on screen.
pub(crate) fn note_shown(label: &str) {
    SHOWN.lock().unwrap().insert(label.to_string());
}

/// The client just took `label` off screen.
pub(crate) fn note_hidden(label: &str) {
    SHOWN.lock().unwrap().remove(label);
}

/// A window is gone for good. Called from `Destroyed`, beside the traffic-light
/// observer's own unwatch, so the set cannot grow one entry per closed window.
pub(crate) fn forget(label: &str) {
    note_hidden(label);
}

/// Did the client put this window on screen and leave it there?
pub(crate) fn shown_by_client(label: &str) -> bool {
    SHOWN.lock().unwrap().contains(label)
}

/// Is the whole app hidden? Always false until [`watch_app_hide`] says
/// otherwise, and always false off macOS, where there is no app-hide.
pub(crate) fn app_is_hidden() -> bool {
    APP_HIDDEN.load(Ordering::SeqCst)
}

/// Record an app-wide hide or unhide.
fn note_app_hidden(hidden: bool) {
    APP_HIDDEN.store(hidden, Ordering::SeqCst);
}

/// Must this window's page be running right now?
///
/// Pure, and the one place the three axes are combined. Our own intent covers
/// the hides the client performs, `app_hidden` covers Cmd-H, and `miniaturized`
/// covers the yellow button. `[NSWindow isVisible]` is deliberately absent: it
/// is the value under suspicion.
pub(crate) fn page_should_be_running(
    shown_by_client: bool,
    app_hidden: bool,
    miniaturized: bool,
) -> bool {
    shown_by_client && !app_hidden && !miniaturized
}

/// Off macOS nothing hides a whole app behind the client's back.
#[cfg(not(target_os = "macos"))]
pub(crate) fn watch_app_hide() {}

/// Track Cmd-H through AppKit's own notifications. Called once, from `setup`,
/// on the main thread.
///
/// One hide is not ours to perform. AppKit orders every window out for Cmd-H
/// without telling the client. Unobserved, the watchdog would read a hidden app
/// as on screen and reload its pages.
///
/// The observers are never removed, which is right: they live as long as the
/// application object they watch.
#[cfg(target_os = "macos")]
pub(crate) fn watch_app_hide() {
    use objc2_foundation::{NSNotification, NSNotificationCenter};

    if objc2::MainThreadMarker::new().is_none() {
        eprintln!("[Tauri] Not on the main thread: the app-hide observer is not installed");
        return;
    }
    let centre = NSNotificationCenter::defaultCenter();
    // SAFETY: reading AppKit's own notification-name constants.
    let names = unsafe {
        [
            (objc2_app_kit::NSApplicationDidHideNotification, true),
            (objc2_app_kit::NSApplicationDidUnhideNotification, false),
        ]
    };
    for (name, hidden) in names {
        let block = block2::RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
            note_app_hidden(hidden);
        });
        // SAFETY: no sender filter, because only the application posts these. A
        // nil queue runs the block on the posting thread, which is the main one.
        let observer = unsafe {
            centre.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        // Deliberately leaked: the pair must outlive every window, and no
        // teardown path could remove them at the right moment.
        std::mem::forget(observer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_runs_only_while_its_window_is_on_screen() {
        // The ordinary case: the client showed it, nothing took it away.
        assert!(page_should_be_running(true, false, false));
        // The three ways a window leaves the screen, each on its own.
        assert!(!page_should_be_running(false, false, false));
        assert!(!page_should_be_running(true, true, false));
        assert!(!page_should_be_running(true, false, true));
    }

    #[test]
    fn showing_and_hiding_move_one_label_without_touching_the_others() {
        note_shown("shown-a");
        note_shown("shown-b");
        assert!(shown_by_client("shown-a"));
        assert!(shown_by_client("shown-b"));
        note_hidden("shown-a");
        assert!(!shown_by_client("shown-a"));
        assert!(shown_by_client("shown-b"), "hiding one must not hide both");
        // A label nobody ever showed is not on screen, which is the safe answer
        // for a window the client has not touched yet.
        assert!(!shown_by_client("shown-never"));
        note_hidden("shown-b");
    }

    #[test]
    fn forgetting_a_destroyed_window_leaves_nothing_behind() {
        note_shown("forget-me");
        forget("forget-me");
        assert!(!shown_by_client("forget-me"));
    }

    #[test]
    fn the_app_hide_flag_survives_a_round_trip() {
        // Off macOS this stays false for good, which the default already gives.
        note_app_hidden(true);
        assert!(app_is_hidden());
        note_app_hidden(false);
        assert!(!app_is_hidden());
    }
}
