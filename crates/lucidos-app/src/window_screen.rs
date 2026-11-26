use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static SHOWN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

static APP_HIDDEN: AtomicBool = AtomicBool::new(false);

pub(crate) fn note_shown(label: &str) {
    SHOWN.lock().unwrap().insert(label.to_string());
}

pub(crate) fn note_hidden(label: &str) {
    SHOWN.lock().unwrap().remove(label);
}

pub(crate) fn forget(label: &str) {
    note_hidden(label);
}

pub(crate) fn shown_by_client(label: &str) -> bool {
    SHOWN.lock().unwrap().contains(label)
}

pub(crate) fn app_is_hidden() -> bool {
    APP_HIDDEN.load(Ordering::SeqCst)
}

#[cfg(any(target_os = "macos", test))]
fn note_app_hidden(hidden: bool) {
    APP_HIDDEN.store(hidden, Ordering::SeqCst);
}

pub(crate) fn page_should_be_running(
    shown_by_client: bool,
    app_hidden: bool,
    miniaturized: bool,
    page_reported_hidden: bool,
    key_window: bool,
) -> bool {
    let webkit_suspended_it = page_reported_hidden && !key_window;
    shown_by_client && !app_hidden && !miniaturized && !webkit_suspended_it
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn watch_app_hide() {}

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
        std::mem::forget(observer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_runs_only_while_its_window_is_on_screen() {
        assert!(page_should_be_running(true, false, false, false, false));
        assert!(!page_should_be_running(false, false, false, false, false));
        assert!(!page_should_be_running(true, true, false, false, false));
        assert!(!page_should_be_running(true, false, true, false, false));
    }

    #[test]
    fn a_covered_window_whose_page_reported_hidden_is_left_alone() {
        assert!(!page_should_be_running(true, false, false, true, false));
    }

    #[test]
    fn the_key_window_is_watched_even_when_its_page_reported_hidden() {
        assert!(page_should_be_running(true, false, false, true, true));
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
        note_app_hidden(true);
        assert!(app_is_hidden());
        note_app_hidden(false);
        assert!(!app_is_hidden());
    }
}
