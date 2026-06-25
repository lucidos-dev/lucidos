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
    use objc2_foundation::{NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
        UNNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };
    use tauri::{AppHandle, Emitter, Manager};

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

                let link = pending().lock().unwrap().remove(&identifier);
                if routed {
                    if let Some(app) = APP.get() {
                        // Any tap (not a dismiss) brings the app forward, even if
                        // the deep link is missing — matches the prior behaviour.
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.unminimize();
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                        if let Some(link) = link {
                            let _ = app.emit("native-notification-tapped", link);
                        }
                    }
                }

                // UN requires the completion handler be invoked once processing
                // is done, or the system logs warnings / may kill the callback.
                completion_handler.call(());
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
}

#[cfg(target_os = "macos")]
pub use imp::{setup, show};

/// Non-macOS desktop has no native notification path (the engine still
/// web-pushes browser / PWA clients); these are no-ops so call sites stay
/// platform-agnostic.
#[cfg(not(target_os = "macos"))]
pub fn setup(_app: &tauri::AppHandle) {}

#[cfg(not(target_os = "macos"))]
pub fn show(_title: &str, _body: &str, _link: serde_json::Value) {}

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
