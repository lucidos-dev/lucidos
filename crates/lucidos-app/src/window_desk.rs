use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::Manager;

#[cfg(any(target_os = "macos", test))]
const DESK_SETTLE: Duration = Duration::from_millis(500);

const DESK_POLL: Duration = Duration::from_millis(250);

static PASS_DUE_AT: Mutex<Option<Instant>> = Mutex::new(None);

#[cfg(target_os = "macos")]
fn note_desk_changed() {
    *PASS_DUE_AT.lock().unwrap() = Some(Instant::now() + DESK_SETTLE);
}

fn claim_due_pass(owed: &mut Option<Instant>, now: Instant) -> bool {
    match *owed {
        Some(due) if now >= due => {
            *owed = None;
            true
        }
        _ => false,
    }
}

fn fit_windows_to_the_desk(app: &tauri::AppHandle) {
    for label in app.windows().into_keys() {
        if crate::app_window::is_app_window(&label) {
            crate::window_restore::clamp_live_geometry(app, &label);
        }
    }
}

pub(crate) fn watch_desk(app: tauri::AppHandle) {
    if !watch_screen_parameters() {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(DESK_POLL);
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

#[cfg(not(target_os = "macos"))]
fn watch_screen_parameters() -> bool {
    false
}

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
    std::mem::forget(observer);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(claim_due_pass(&mut owed, start + DESK_SETTLE * 2));
    }

    #[test]
    fn a_claimed_pass_is_not_re_taken() {
        let start = Instant::now();
        let mut owed = Some(start);
        assert!(claim_due_pass(&mut owed, start));
        assert_eq!(owed, None);
        assert!(!claim_due_pass(&mut owed, start + DESK_SETTLE * 10));
    }

    #[test]
    fn an_unchanged_desk_owes_no_pass() {
        let mut owed = None;
        assert!(!claim_due_pass(&mut owed, Instant::now()));
    }

    #[test]
    fn a_pass_waits_for_the_quiet_period() {
        let start = Instant::now();
        let mut owed = Some(start + DESK_SETTLE);
        assert!(!claim_due_pass(&mut owed, start));
        assert!(!claim_due_pass(&mut owed, start + DESK_SETTLE - DESK_POLL));
        assert!(claim_due_pass(&mut owed, start + DESK_SETTLE));
    }
}
