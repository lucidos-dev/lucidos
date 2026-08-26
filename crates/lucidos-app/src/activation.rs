//! Whether the client is a normal Dock app or lives in the menu bar, and the
//! unread count each of those surfaces shows.
//!
//! Two facts and the rules over them. The activation policy decides whether
//! there is a Dock tile and a Cmd-Tab entry at all. The unread total then goes
//! wherever it can be seen: the tray title always, the Dock badge only while a
//! tile exists.
//!
//! The predicates here take a COUNT rather than an app handle, so they stay
//! pure. Counting the visible windows belongs to whoever owns the window set.

use std::sync::atomic::{AtomicBool, AtomicU64};

use crate::notifications;

/// True while the packaged macOS client is running menu-bar-only: no visible app
/// window, the `Accessory` activation policy, and absent from the Dock and
/// Cmd+Tab. [`apply_unread_indicator`] reads it to tell whether there is a Dock
/// tile to badge, since the tray carries the count either way. Starts `false`,
/// so an ordinary launch is a normal `Regular` Dock app.
static MENU_BAR_ONLY: AtomicBool = AtomicBool::new(false);

/// The most recent aggregate unread total. An activation-policy transition
/// re-applies the SAME count to the Dock tile that just appeared or vanished,
/// rather than waiting for the next badge-loop poll.
static LAST_UNREAD: AtomicU64 = AtomicU64::new(0);

/// Is the client menu-bar-only right now?
pub(crate) fn is_menu_bar_only() -> bool {
    MENU_BAR_ONLY.load(std::sync::atomic::Ordering::SeqCst)
}

/// The client should be menu-bar-only exactly when no app window is visible.
pub(crate) fn should_be_menu_bar_only(visible_app_windows: usize) -> bool {
    visible_app_windows == 0
}

/// Should a macOS *reopen* put a window back on screen? Only when the user can
/// see none, which is the one thing
/// `applicationShouldHandleReopen:hasVisibleWindows:` exists to answer.
///
/// **With a window already up, a reopen must change nothing**, and that is a fix
/// rather than a tidy-up. macOS raises the event for a Dock-icon click, a Finder
/// re-open, and an activation driven by a notification tap. The Dock activates
/// the app by itself either way. Answering any of them by fronting `main` raises
/// whatever workspace `main` is pointed at, over the window the user was on.
/// That is the rule `focus_calling_window` already states, and a native banner
/// tap is where it bit: `route_native_tap` picks the window on the workspace
/// that raised the banner, and the reopen then overruled it.
///
/// Compiled off macOS only for its test: `RunEvent::Reopen` is macOS-only.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn reopen_shows_a_window(visible_app_windows: usize) -> bool {
    visible_app_windows == 0
}

/// Which surfaces show the unread `count`, as `(dock_badge_label, tray_title)`.
/// The tray title ALWAYS carries the count. The Dock badge carries it while the
/// client is a `Regular` Dock app, and stays `None` while menu-bar-only, where
/// there is no tile to badge. Zero clears both, and a count over 99 collapses
/// to "99+".
///
/// **The two halves clear differently, which is why their types differ.** The
/// Dock badge is an `Option`, because `setBadgeLabel(None)` removes the bubble.
/// The tray title is a `String` whose EMPTY value means no count. `tray-icon`'s
/// macOS `set_title` ignores a `None` outright. A cleared title must therefore
/// be WRITTEN empty, or the menu bar keeps the last number it was given.
fn unread_targets(menu_bar_only: bool, count: u64) -> (Option<String>, String) {
    if count == 0 {
        return (None, String::new());
    }
    let label = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    let dock = if menu_bar_only {
        None
    } else {
        Some(label.clone())
    };
    (dock, label)
}

/// Show the aggregate unread `count` on every surface that exists right now.
/// Stores it in [`LAST_UNREAD`] so a later activation transition can re-apply it
/// to the Dock tile that just appeared or vanished.
pub(crate) fn apply_unread_indicator(app: &tauri::AppHandle, count: u64) {
    LAST_UNREAD.store(count, std::sync::atomic::Ordering::SeqCst);
    let (dock, tray) = unread_targets(is_menu_bar_only(), count);
    notifications::set_dock_badge(dock);
    notifications::set_tray_title(app, &tray);
}

/// Switch the client between menu-bar-only and a normal Dock app. On macOS this
/// sets the NSApplication activation policy; elsewhere only the flag moves. Then
/// re-applies the unread indicator, so the Dock tile that just appeared or
/// vanished agrees with the tray. See
/// `docs/plans/2026-07-01-macos-client-menu-bar-only-on-window-close.md`.
pub(crate) fn set_menu_bar_only(app: &tauri::AppHandle, menu_bar_only: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if menu_bar_only {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        };
        if let Err(e) = app.set_activation_policy(policy) {
            eprintln!("[Tauri] Failed to set activation policy: {e}");
        }
    }
    MENU_BAR_ONLY.store(menu_bar_only, std::sync::atomic::Ordering::SeqCst);
    apply_unread_indicator(app, LAST_UNREAD.load(std::sync::atomic::Ordering::SeqCst));
}

/// Activate the app frontmost on macOS. No-op elsewhere.
pub(crate) fn activate_app_frontmost() {
    #[cfg(target_os = "macos")]
    notifications::activate_app();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_be_menu_bar_only_iff_no_windows_visible() {
        // No visible app window, so drop to the menu-bar tray (Accessory).
        assert!(should_be_menu_bar_only(0));
        // Any visible window keeps it a normal Dock app (Regular), including
        // the main-hidden-but-a-secondary-still-open case.
        assert!(!should_be_menu_bar_only(1));
        assert!(!should_be_menu_bar_only(3));
    }

    #[test]
    fn a_reopen_only_shows_a_window_when_none_is_on_screen() {
        // Trayed / windowless: a Dock click is the way back, so show one.
        assert!(reopen_shows_a_window(0));
        // The bug: with a window up, fronting `main` raised whatever workspace
        // it was on over the one the user was looking at. A native banner tap
        // reaches here through the reopen macOS raises when it activates the
        // app, right after `route_native_tap` chose the raising workspace's
        // window. Leave the order alone.
        assert!(!reopen_shows_a_window(1));
        assert!(!reopen_shows_a_window(2));
    }

    #[test]
    fn unread_targets_always_titles_the_tray_and_badges_the_dock_only_under_regular() {
        // A window is open, so the count is on BOTH the Dock badge and the tray
        // title. The menu bar carrying it at all times is the point.
        assert_eq!(
            unread_targets(false, 5),
            (Some("5".into()), "5".to_string())
        );
        // Menu-bar only (Accessory): tray title only, because there is no Dock tile.
        assert_eq!(unread_targets(true, 5), (None, "5".to_string()));
        // Over 99 collapses to "99+" on every surface that shows it.
        assert_eq!(
            unread_targets(false, 100),
            (Some("99+".into()), "99+".to_string())
        );
        assert_eq!(
            unread_targets(false, 150),
            (Some("99+".into()), "99+".to_string())
        );
        assert_eq!(unread_targets(true, 100), (None, "99+".to_string()));
        // Boundary: exactly 99 is shown as-is, on both surfaces.
        assert_eq!(
            unread_targets(false, 99),
            (Some("99".into()), "99".to_string())
        );
        assert_eq!(unread_targets(true, 99), (None, "99".to_string()));
    }

    #[test]
    fn a_cleared_tray_title_is_an_empty_string_the_tray_will_actually_write() {
        // Zero has to reach `set_tray_title` as a value it WRITES, because
        // `tray-icon`'s macOS backend drops a `None` and leaves the last count
        // on screen. An empty string takes the same path a real count does, so
        // the item blanks.
        for menu_bar_only in [false, true] {
            let (dock, tray) = unread_targets(menu_bar_only, 0);
            assert_eq!(dock, None, "the Dock badge clears with None, which works");
            assert_eq!(
                tray, "",
                "the tray clears by being WRITTEN empty, not skipped"
            );
        }
        // And the non-zero case must not be empty, or clearing would be
        // indistinguishable from showing a count.
        assert_ne!(unread_targets(false, 1).1, "");
    }
}
