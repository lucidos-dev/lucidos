//! Native macOS notifications via Apple's modern UserNotifications framework
//! (`UNUserNotificationCenter`).
//!
//! Replaces the deprecated `NSUserNotification` path (`mac-notification-sys`),
//! which Apple has dismantled — it no longer delivers on recent macOS (26
//! "Tahoe"). `UNUserNotification` is the supported API, but it **requires the
//! process to run inside a signed `.app` bundle**: `currentNotificationCenter()`
//! throws for an unbundled binary. A `tauri dev` build is an unbundled
//! `cargo run` binary, so native banners are inert in dev — `tauri::is_dev()`
//! short-circuits both [`setup`] and [`show`]. Browser / PWA clients still get
//! the web push on the same engine branch, so dev keeps a working notification
//! channel. A packaged build (`cargo tauri build`) is a real bundle and
//! delivers. See `system-knowhow/notifications.md` §4.
//!
//! Tap routing is delegate-based (UN has no synchronous "wait for click" like
//! the old crate): a `UNUserNotificationCenterDelegate` receives the tap on the
//! main thread, looks up the deep link stashed at [`show`] time (keyed by the
//! notification's identifier), focuses the window, and emits
//! `native-notification-tapped` — the SAME event + payload shape the page
//! already routes through `dispatchDeepLink` (`store/actions/native-push.ts`).

/// The notification request identifier carried by a deep link: the engine
/// `notification_id` string, or `""` when absent / not a string. It keys the
/// pending-link map and IS the UN request identifier, so a repeat for the same
/// notification REPLACES its banner (matching the web-push `tag`). Pure +
/// platform-independent so the extraction is unit-testable off macOS.
#[cfg(any(target_os = "macos", test))]
fn link_identifier(link: &serde_json::Value) -> String {
    link.get("notification_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// True when a UN response action identifier is the swipe-away dismiss
/// pseudo-action (`…DismissActionIdentifier`) rather than a real tap — a dismiss
/// must drop its stashed deep link WITHOUT routing or bringing the app forward.
/// Pure + platform-independent so it is testable off macOS.
#[cfg(any(target_os = "macos", test))]
fn is_dismiss_action(action: &str) -> bool {
    action.ends_with("DismissActionIdentifier")
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AllocAnyThread};
    use objc2_foundation::{NSArray, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };
    use tauri::{AppHandle, Emitter};

    /// App handle the delegate uses to focus the window + emit the tap event.
    /// Set once in [`setup`]; read on the main thread in the tap callback.
    static APP: OnceLock<AppHandle> = OnceLock::new();

    /// Soft cap on un-acted pending deep links. Past this, the map is cleared
    /// rather than grown unbounded — the cost of an overflow eviction is that a
    /// very old, never-tapped notification's tap falls back to just focusing the
    /// app (no deep link). Lucidos's notification volume is low, so this is a
    /// safety valve, not a routine path.
    const MAX_PENDING: usize = 256;

    /// Deep links awaiting a tap, keyed by the notification request identifier
    /// (= the engine `notification_id`). Set at [`show`], consumed on tap.
    /// In-process only — a tap after the app fully quit can't route (matches the
    /// old `mac-notification-sys` behaviour; the bell badge driven by
    /// `NotificationCreated` stays the durable signal). See notifications.md §4.
    fn pending() -> &'static Mutex<HashMap<String, serde_json::Value>> {
        static P: OnceLock<Mutex<HashMap<String, serde_json::Value>>> = OnceLock::new();
        P.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Deep links from taps the page may not have been listening for at emit
    /// time — webview reloaded / suspended-while-trayed / client relaunched.
    /// The live `app.emit` is a best-effort warm-path wake; this stash is the
    /// DURABLE carrier, drained by the page via [`take_pending_taps`] both on
    /// startup (cold path) and on each `native-notification-tapped` signal (warm
    /// path). The Mutex makes each drain atomic, so a tap routes exactly once
    /// across both paths and never re-fires on a later reload. Capped like
    /// [`pending`] (FIFO drop) so a long-resident client whose signals are all
    /// missed can't grow it unbounded. See `store/actions/native-push.ts` and
    /// `system-knowhow/notifications.md` §4.
    fn pending_taps() -> &'static Mutex<Vec<serde_json::Value>> {
        static T: OnceLock<Mutex<Vec<serde_json::Value>>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(Vec::new()))
    }

    define_class!(
        // Plain NSObject subclass: no ivars (state lives in the module statics so
        // the leaked, weakly-referenced delegate stays trivially constructible).
        #[unsafe(super(NSObject))]
        #[name = "LucidosNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive_response(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion_handler: &block2::DynBlock<dyn Fn()>,
            ) {
                let identifier = response.notification().request().identifier().to_string();
                // The dismiss pseudo-action ("…DismissActionIdentifier") is a
                // swipe-away, not a tap — drop its stashed link without routing.
                let action = response.actionIdentifier().to_string();
                let routed = !super::is_dismiss_action(&action);

                let link = pending().lock().unwrap().remove(&identifier).or_else(|| {
                    // Relaunch / evicted-map fallback: the UN request identifier
                    // IS the engine notification_id, and a modal-default tap only
                    // needs that to open the notification detail. So a tap on a
                    // banner from a previous client process (empty in-process map)
                    // still routes to the inbox modal — graceful degradation: a
                    // navigate-kind tap falls back to modal. Empty id → nothing.
                    (!identifier.is_empty())
                        .then(|| serde_json::json!({ "notification_id": identifier }))
                });
                eprintln!(
                    "[Tauri] native notification tap: id={identifier:?} routed={routed} \
                     link_present={}",
                    link.is_some()
                );
                if routed {
                    if let Some(app) = APP.get() {
                        // Any tap (not a dismiss) brings the app forward, even if
                        // the deep link is missing — matches the prior behaviour.
                        // show_main_window also emits `native-window-active = true`
                        // so the reshown page counts as active again.
                        crate::show_main_window(app);
                        if let Some(link) = link {
                            // Stash for DURABLE delivery, then fire the warm-path
                            // wake. The page drains the stash (take_pending_taps)
                            // on BOTH the startup cold path and this signal, so the
                            // tap routes exactly once even if this emit lands on a
                            // page whose listener isn't registered yet.
                            {
                                let mut taps = pending_taps().lock().unwrap();
                                if taps.len() >= MAX_PENDING {
                                    taps.remove(0);
                                }
                                taps.push(link.clone());
                            }
                            let _ = app.emit("native-notification-tapped", link);
                        }
                    }
                }

                // UN requires the completion handler be invoked once processing
                // is done, or the system logs warnings / may kill the callback.
                completion_handler.call(());
            }

            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                // macOS suppresses a banner for the FRONTMOST app unless the
                // delegate opts in here. The engine only emits NativePushRequested
                // (→ the page's show()) on the no-active-device branch, so a show()
                // always SHOULD surface — return the banner even if the window
                // happens to be frontmost (e.g. a stale active-state signal got it
                // wrong, the inverse of the "only sometimes" miss). Safe: a banner
                // and an in-app toast can't both fire for one notification (engine
                // gates them on opposite branches). See notifications.md §4.
                completion_handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }
        }
    );

    fn new_delegate() -> Retained<Delegate> {
        unsafe { msg_send![Delegate::alloc(), init] }
    }

    /// Wire the delegate + request authorization once, at app startup. No-op in
    /// dev: `tauri dev` runs an unbundled binary, where
    /// `currentNotificationCenter()` throws an Objective-C exception (which would
    /// abort the process), so we never touch UN there.
    pub fn setup(app: &AppHandle) {
        if tauri::is_dev() {
            eprintln!(
                "[Tauri] native notifications disabled in dev (unbundled binary); \
                 browser/PWA clients still receive web push"
            );
            return;
        }
        let _ = APP.set(app.clone());
        let center = UNUserNotificationCenter::currentNotificationCenter();

        // `setDelegate` is a WEAK property, so the delegate must outlive this
        // function. We deliberately leak one object for the app's lifetime
        // (there is exactly one delegate, created once) rather than thread a
        // non-Send `Retained` through a static.
        let delegate = new_delegate();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        std::mem::forget(delegate);

        let opts = UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge;
        let handler = RcBlock::new(|granted: Bool, _err: *mut NSError| {
            if !granted.as_bool() {
                eprintln!("[Tauri] native notification permission not granted");
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(opts, &handler);
        // This is an *escaping* completion handler: UN invokes it after `setup`
        // returns (when the user answers the first-launch prompt). The framework
        // copies the block per ObjC convention, but this obj-c path can't be
        // runtime-tested here, so we also leak our `RcBlock` (one tiny one-shot
        // block per launch) to guarantee it outlives the async callback under any
        // copy semantics — same rationale as the delegate leak above.
        std::mem::forget(handler);
    }

    /// Show a banner. `link` is the SW-message-shaped deep link emitted back on
    /// tap. No-op in dev (see [`setup`]). The `notification_id` from `link` is
    /// the request identifier, so a repeat for the same notification REPLACES its
    /// banner (matching the web-push `tag`) and keys the pending-link map.
    pub fn show(title: &str, body: &str, link: serde_json::Value) {
        if tauri::is_dev() {
            return;
        }
        let identifier = super::link_identifier(&link);

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));

        if !identifier.is_empty() {
            let mut map = pending().lock().unwrap();
            if map.len() >= MAX_PENDING {
                map.clear();
            }
            map.insert(identifier.clone(), link);
        }

        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&identifier),
            &content,
            None,
        );
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, None);
    }

    /// Remove already-delivered native banner(s) — the cross-device dismiss
    /// counterpart of [`show`]. `id = Some(notification_id)` removes the one
    /// banner whose request identifier matches (set at [`show`] time); `None`
    /// removes ALL delivered banners (the mark-all-read path). Also drops the
    /// stashed deep link(s) from [`pending`] so a tap on a now-removed banner
    /// can't route. No-op in dev (see [`setup`]). An empty `Some("")` is a
    /// malformed single id → no-op (never an accidental dismiss-all), matching
    /// [`show`]'s empty-identifier handling.
    ///
    /// UN's remove methods are safe to call from any thread (like
    /// `addNotificationRequest` in [`show`]), so no main-thread marshaling.
    pub fn dismiss(id: Option<String>) {
        if tauri::is_dev() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        match id {
            Some(identifier) if !identifier.is_empty() => {
                let ids = NSArray::from_retained_slice(&[NSString::from_str(&identifier)]);
                center.removeDeliveredNotificationsWithIdentifiers(&ids);
                pending().lock().unwrap().remove(&identifier);
            }
            // Empty single id — nothing actionable; do NOT fall through to all.
            Some(_) => {}
            None => {
                center.removeAllDeliveredNotifications();
                pending().lock().unwrap().clear();
            }
        }
    }

    /// Set the dock-icon badge to `label`, or clear it with `None`. The caller
    /// (`crate::apply_unread_indicator`) formats the AGGREGATE unread total across
    /// running workspaces (the Tauri window fronts the gateway, so its app icon
    /// represents all workspaces) — including the `0`→clear and `>99`→"99+" rules —
    /// and routes it here only while the client is a normal `Regular` Dock app; a
    /// menu-bar-only client has no Dock tile and shows the count via
    /// [`set_tray_title`] instead.
    ///
    /// MUST be called on the main thread — `MainThreadMarker::new()` returns
    /// `None` off it and we bail (the desktop poll marshals here via
    /// `run_on_main_thread`). Unlike [`setup`]/[`show`] this works in dev too: the
    /// dock tile exists for an unbundled `cargo run` app (only UN needs a bundle).
    pub fn set_dock_badge(label: Option<String>) {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        match label {
            Some(l) => tile.setBadgeLabel(Some(&NSString::from_str(&l))),
            None => tile.setBadgeLabel(None),
        }
    }

    /// Set (or clear, with `None`) the menu-bar tray icon's title text. Used to show
    /// the unread count next to the icon while the client is menu-bar-only, where
    /// there is no Dock tile to badge (`crate::apply_unread_indicator` routes the
    /// count here vs [`set_dock_badge`] based on the activation policy). Looks up
    /// the tray by the id it was built with (`lucidos-tray`). Best-effort: a missing
    /// tray / failure is logged, not fatal.
    pub fn set_tray_title(app: &AppHandle, title: Option<String>) {
        if let Some(tray) = app.tray_by_id("lucidos-tray") {
            if let Err(e) = tray.set_title(title.as_deref()) {
                eprintln!("[Tauri] Failed to set tray title: {e}");
            }
        }
    }

    /// Bring the app to the foreground. Needed after leaving the `Accessory`
    /// activation policy: switching back to `Regular` alone can leave the app
    /// behind other apps with an unclickable menu bar (a known AppKit gotcha), so
    /// the reopen path explicitly activates. Must run on the main thread (all
    /// callers do — tray menu / notification tap / Reopen).
    pub fn activate_app() {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // `activateIgnoringOtherApps:` is deprecated as of macOS 14 in favor of the
        // parameterless `activate()`, but that exists only on macOS 14+ and this app
        // targets macOS 11+ (see tauri.conf.json), so keep the cross-version call.
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }

    /// Drain (and clear) every deep link stashed by a tap the page may not have
    /// been listening for. The frontend calls this on startup (cold path) AND on
    /// each `native-notification-tapped` signal (warm path); the Mutex makes the
    /// drain atomic so each tap routes exactly once across both paths and can't
    /// re-fire on a later reload. Naturally empty in dev (the delegate never
    /// fires) and once every tap has been consumed. See `native-push.ts`.
    pub fn take_pending_taps() -> Vec<serde_json::Value> {
        std::mem::take(&mut *pending_taps().lock().unwrap())
    }
}

#[cfg(target_os = "macos")]
pub use imp::{
    activate_app, dismiss, set_dock_badge, set_tray_title, setup, show, take_pending_taps,
};

/// Non-macOS desktop has no native notification path (the engine still
/// web-pushes browser / PWA clients); these are no-ops so call sites stay
/// platform-agnostic.
#[cfg(not(target_os = "macos"))]
pub fn setup(_app: &tauri::AppHandle) {}

#[cfg(not(target_os = "macos"))]
pub fn show(_title: &str, _body: &str, _link: serde_json::Value) {}

/// No native banner removal off macOS (no native notification path here); a
/// no-op so the Tauri command stays platform-agnostic.
#[cfg(not(target_os = "macos"))]
pub fn dismiss(_id: Option<String>) {}

/// No dock badge off macOS (the native app-icon badge is a macOS dock-tile
/// concept). Browser / PWA clients still get the Badging API on every platform.
#[cfg(not(target_os = "macos"))]
pub fn set_dock_badge(_label: Option<String>) {}

/// No tray-title unread count off macOS — the menu-bar-only activation model is a
/// macOS concept; a no-op so `crate::apply_unread_indicator` stays platform-agnostic.
#[cfg(not(target_os = "macos"))]
pub fn set_tray_title(_app: &tauri::AppHandle, _title: Option<String>) {}

/// No native tap stash off macOS (no native notification path here); always
/// empty so the Tauri command stays platform-agnostic.
#[cfg(not(target_os = "macos"))]
pub fn take_pending_taps() -> Vec<serde_json::Value> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn link_identifier_reads_the_notification_id_string() {
        // Present → the id IS the request identifier (replace-by-id semantics).
        assert_eq!(
            link_identifier(&json!({"notification_id": "abc-123", "thread_id": "t1"})),
            "abc-123"
        );
        // Absent → empty (the caller then skips the pending-link insert).
        assert_eq!(link_identifier(&json!({"thread_id": "t1"})), "");
        // Non-string → empty (defensive: a number/null is not a usable id).
        assert_eq!(link_identifier(&json!({"notification_id": 42})), "");
        assert_eq!(link_identifier(&json!({"notification_id": null})), "");
        assert_eq!(link_identifier(&serde_json::Value::Null), "");
    }

    #[test]
    fn is_dismiss_action_only_matches_the_swipe_away_pseudo_action() {
        // The system dismiss pseudo-action → true (drop the link, don't route).
        assert!(is_dismiss_action(
            "com.apple.UNNotificationDismissActionIdentifier"
        ));
        // The default tap action → false (route the deep link).
        assert!(!is_dismiss_action(
            "com.apple.UNNotificationDefaultActionIdentifier"
        ));
        // A custom action id → false.
        assert!(!is_dismiss_action("my.custom.action"));
        assert!(!is_dismiss_action(""));
    }
}
