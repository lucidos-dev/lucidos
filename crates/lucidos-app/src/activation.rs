use std::sync::atomic::{AtomicBool, AtomicU64};

use crate::notifications;

static MENU_BAR_ONLY: AtomicBool = AtomicBool::new(false);

static LAST_UNREAD: AtomicU64 = AtomicU64::new(0);

pub(crate) fn is_menu_bar_only() -> bool {
    MENU_BAR_ONLY.load(std::sync::atomic::Ordering::SeqCst)
}

pub(crate) fn should_be_menu_bar_only(visible_app_windows: usize) -> bool {
    visible_app_windows == 0
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn reopen_shows_a_window(visible_app_windows: usize) -> bool {
    visible_app_windows == 0
}

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

pub(crate) fn apply_unread_indicator(app: &tauri::AppHandle, count: u64) {
    LAST_UNREAD.store(count, std::sync::atomic::Ordering::SeqCst);
    let (dock, tray) = unread_targets(is_menu_bar_only(), count);
    notifications::set_dock_badge(dock);
    notifications::set_tray_title(app, &tray);
}

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

pub(crate) fn activate_app_frontmost() {
    #[cfg(target_os = "macos")]
    notifications::activate_app();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_be_menu_bar_only_iff_no_windows_visible() {
        assert!(should_be_menu_bar_only(0));
        assert!(!should_be_menu_bar_only(1));
        assert!(!should_be_menu_bar_only(3));
    }

    #[test]
    fn a_reopen_only_shows_a_window_when_none_is_on_screen() {
        assert!(reopen_shows_a_window(0));
        assert!(!reopen_shows_a_window(1));
        assert!(!reopen_shows_a_window(2));
    }

    #[test]
    fn unread_targets_always_titles_the_tray_and_badges_the_dock_only_under_regular() {
        assert_eq!(
            unread_targets(false, 5),
            (Some("5".into()), "5".to_string())
        );
        assert_eq!(unread_targets(true, 5), (None, "5".to_string()));
        assert_eq!(
            unread_targets(false, 100),
            (Some("99+".into()), "99+".to_string())
        );
        assert_eq!(
            unread_targets(false, 150),
            (Some("99+".into()), "99+".to_string())
        );
        assert_eq!(unread_targets(true, 100), (None, "99+".to_string()));
        assert_eq!(
            unread_targets(false, 99),
            (Some("99".into()), "99".to_string())
        );
        assert_eq!(unread_targets(true, 99), (None, "99".to_string()));
    }

    #[test]
    fn a_cleared_tray_title_is_an_empty_string_the_tray_will_actually_write() {
        for menu_bar_only in [false, true] {
            let (dock, tray) = unread_targets(menu_bar_only, 0);
            assert_eq!(dock, None, "the Dock badge clears with None, which works");
            assert_eq!(
                tray, "",
                "the tray clears by being WRITTEN empty, not skipped"
            );
        }
        assert_ne!(unread_targets(false, 1).1, "");
    }
}
