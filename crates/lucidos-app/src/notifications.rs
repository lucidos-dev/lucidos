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

#[cfg(any(target_os = "macos", test))]
use crate::window_target::{
    is_workspace_slug, labels_in, preferred_label, window_context, window_origin, workspace_url,
    WindowContext,
};

/// Separator between the workspace slug and the engine notification id inside a
/// UN request identifier. Safe on both sides: a uuid never contains it, and a
/// gateway slug is `[a-z0-9]+(-[a-z0-9]+)*` (`SLUG_RE` in `utils/basePath.ts`).
#[cfg(any(target_os = "macos", test))]
const IDENTIFIER_SEPARATOR: char = '|';

/// The UN request identifier for a notification: `<workspace>|<notification_id>`,
/// or the bare id when there is no workspace (a legacy direct engine served with
/// no gateway, where `WORKSPACE_ID` is null and there is only one workspace).
///
/// **The workspace has to live in the IDENTIFIER, not only in the stashed deep
/// link.** One packaged client process fronts the gateway and can point any
/// window at any workspace (ADR 0014), so a tap has to say which workspace
/// raised it. The stash is in-process and empty after a relaunch; the identifier
/// travels inside the delivered notification, so it is what [`split_identifier`]
/// can still recover then. It also keeps replace-by-id (the web-push `tag`
/// equivalent) scoped: a repeat from the SAME workspace replaces its banner, and
/// two workspaces can never collide.
///
/// An empty `notification_id` yields an empty identifier whatever the workspace,
/// so [`show`]'s `is_empty` guard keeps meaning "no usable id".
///
/// Pure + platform-independent so it is unit-testable off macOS.
#[cfg(any(target_os = "macos", test))]
fn notification_identifier(workspace: &str, notification_id: &str) -> String {
    if notification_id.is_empty() {
        return String::new();
    }
    if workspace.is_empty() {
        return notification_id.to_string();
    }
    format!("{workspace}{IDENTIFIER_SEPARATOR}{notification_id}")
}

/// Inverse of [`notification_identifier`]: `(workspace, notification_id)`.
/// `None` workspace for a bare identifier (no gateway, or a banner delivered by
/// a build older than the composite). Splits on the FIRST separator, so a stray
/// one inside an id still yields the right workspace.
#[cfg(any(target_os = "macos", test))]
fn split_identifier(identifier: &str) -> (Option<&str>, &str) {
    match identifier.split_once(IDENTIFIER_SEPARATOR) {
        Some((workspace, id)) if !workspace.is_empty() => (Some(workspace), id),
        Some((_, id)) => (None, id),
        None => (None, identifier),
    }
}

/// Does a delivered banner's identifier belong to `workspace` (`""` = the
/// no-gateway single workspace)? Drives the scoped dismiss-all, so a
/// mark-all-read in one workspace leaves every other workspace's banners alone.
///
/// A bare identifier belongs only to the no-gateway case, which means a banner
/// left over from a build older than the composite is NOT swept by a
/// workspace's dismiss-all. It stays until the user swipes it: strictly better
/// than the blunt `removeAllDeliveredNotifications` that used to clear every
/// workspace's, and it ages out with the next notification.
#[cfg(any(target_os = "macos", test))]
fn identifier_belongs_to(identifier: &str, workspace: &str) -> bool {
    match split_identifier(identifier).0 {
        Some(owner) => owner == workspace,
        None => workspace.is_empty(),
    }
}

/// A string field of a deep link, or `""` when absent / not a string.
#[cfg(any(target_os = "macos", test))]
fn link_field<'a>(link: &'a serde_json::Value, key: &str) -> &'a str {
    link.get(key).and_then(|v| v.as_str()).unwrap_or_default()
}

/// The UN request identifier a deep link maps to. The page stamps `workspace`
/// (its gateway slug) alongside `notification_id` at [`show`] time; see
/// [`notification_identifier`] for why both are needed.
#[cfg(any(target_os = "macos", test))]
fn link_identifier(link: &serde_json::Value) -> String {
    notification_identifier(
        link_field(link, "workspace"),
        link_field(link, "notification_id"),
    )
}

/// True when a UN response action identifier is the swipe-away dismiss
/// pseudo-action (`…DismissActionIdentifier`) rather than a real tap — a dismiss
/// must drop its stashed deep link WITHOUT routing or bringing the app forward.
/// Pure + platform-independent so it is testable off macOS.
#[cfg(any(target_os = "macos", test))]
fn is_dismiss_action(action: &str) -> bool {
    action.ends_with("DismissActionIdentifier")
}

/// The workspace a deep link names, or `None` when it carries none (a legacy
/// direct engine with no gateway, or a banner delivered by a build older than
/// the stamp). `""` and a non-string both read as absent.
#[cfg(any(target_os = "macos", test))]
fn link_workspace(link: &serde_json::Value) -> Option<&str> {
    let workspace = link_field(link, "workspace");
    (!workspace.is_empty()).then_some(workspace)
}

/// May the page serving `workspace` take this stashed tap? True for its OWN
/// taps, and for an unattributable one (no workspace on the link), which is the
/// legacy / pre-stamp case there is nothing better to do with.
///
/// This is what makes the process-global stash safe to share across windows:
/// every window drains with its own slug, so a tap raised by one workspace is
/// LEFT IN PLACE by every other window's drain rather than consumed by whichever
/// page happened to wake first. Before it, the drain was an unconditional take
/// and which window handled a tap was a race.
#[cfg(any(target_os = "macos", test))]
fn tap_belongs_to(link: &serde_json::Value, workspace: Option<&str>) -> bool {
    match link_workspace(link) {
        None => true,
        Some(owner) => Some(owner) == workspace,
    }
}

/// The origin to build tap-target URLs on: the first window actually served off
/// one. `None` when no window is (every window unnavigated, or none open), in
/// which case the caller falls back to this install's stable gateway URL.
///
/// Compiled on every platform, because a tap is no longer its only caller: the
/// reopen builds its restored windows on the same origin, and it is not
/// macOS-only. The body is a fold over `window_origin`, which is platform
/// independent, so widening this costs nothing.
pub(crate) fn gateway_origin(windows: &[(String, String)]) -> Option<&str> {
    windows.iter().find_map(|(_, url)| window_origin(url))
}

/// Where a native banner tap should land. Decided in Rust because only the
/// client process can see every window, read what workspace each is on, and
/// create one; a page can only ever navigate ITSELF, which is how a tap used to
/// yank a window off the workspace the user had it on.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TapTarget {
    /// This window is already on the raising workspace: show + focus it.
    Focus(String),
    /// This window is on no workspace (picker / root): point it at `url`.
    Navigate { label: String, url: String },
    /// Every window is on some OTHER workspace: open a fresh one at `url`.
    NewWindow { url: String },
    /// The client is still booting, so its first navigation is ours to aim.
    LaunchInto { url: String },
    /// The tap names no workspace: bring the main window forward, as before.
    MainWindow,
}

/// Pick the window a tap raised by `owner` belongs in, given every top-level app
/// window as `(label, url)`.
///
/// **This is the native counterpart of `clientInScope` + `clients.openWindow` in
/// `public/sw.js`**, and it enforces the same rule the service worker does for
/// web push: a tap lands in the workspace that RAISED it, or in a window opened
/// for it, and never in a window sitting on a different workspace.
///
/// Priority:
///  1. no `owner` (legacy / pre-stamp banner) → the main window, as before;
///  2. a window already on `owner` → focus it;
///  3. `main` not navigated yet → aim the boot navigation at `owner`;
///  4. a picker / root window → point it at `owner`;
///  5. otherwise → a new window.
///
/// A malformed `owner` is treated as no owner rather than used to build a URL.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn choose_tap_target(
    windows: &[(String, String)],
    owner: Option<&str>,
    origin: &str,
) -> TapTarget {
    let Some(owner) = owner.filter(|o| is_workspace_slug(o)) else {
        return TapTarget::MainWindow;
    };
    let url = workspace_url(origin, owner);

    if let Some(label) = preferred_label(&labels_in(windows, WindowContext::Workspace(owner))) {
        return TapTarget::Focus(label.to_string());
    }
    // Deliberately BEFORE the neutral branch: `main` is unnavigated only while
    // `desktop::launch` is still waiting on the gateway, and its pending
    // navigation would clobber anything we pointed another window at.
    if windows.iter().any(|(label, url)| {
        label == crate::app_window::MAIN_WINDOW_LABEL
            && window_context(url) == WindowContext::Unnavigated
    }) {
        return TapTarget::LaunchInto { url };
    }
    if let Some(label) = preferred_label(&labels_in(windows, WindowContext::Neutral)) {
        return TapTarget::Navigate {
            label: label.to_string(),
            url,
        };
    }
    TapTarget::NewWindow { url }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::ptr::NonNull;
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
    /// (`<workspace>|<notification_id>`, see [`super::notification_identifier`]).
    /// Set at [`show`], consumed on tap.
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
                    // carries the workspace AND the engine notification_id, and a
                    // modal-default tap only needs those to open the notification
                    // detail in the right workspace. So a tap on a banner from a
                    // previous client process (empty in-process map) still routes
                    // to the inbox modal, degrading gracefully: a navigate-kind
                    // tap falls back to modal. Rebuilding the workspace here is
                    // what keeps the fallback ATTRIBUTABLE: without it the page
                    // would dispatch it into whatever workspace happens to be
                    // loaded. Empty id → nothing.
                    let (workspace, notification_id) = super::split_identifier(&identifier);
                    (!notification_id.is_empty()).then(|| {
                        serde_json::json!({
                            "notification_id": notification_id,
                            "workspace": workspace,
                        })
                    })
                });
                eprintln!(
                    "[Tauri] native notification tap: id={identifier:?} routed={routed} \
                     link_present={}",
                    link.is_some()
                );
                if routed {
                    if let Some(app) = APP.get() {
                        // STASH FIRST, then touch a window. Showing or focusing one
                        // fires that page's `focus` / `visibilitychange` drains, and
                        // a drain that runs before the stash lands finds nothing and
                        // the tap is lost. The stash is the DURABLE carrier; the
                        // emit below is only a warm-path wake.
                        let owner = link
                            .as_ref()
                            .and_then(super::link_workspace)
                            .map(str::to_string);
                        if let Some(link) = &link {
                            let mut taps = pending_taps().lock().unwrap();
                            if taps.len() >= MAX_PENDING {
                                taps.remove(0);
                            }
                            taps.push(link.clone());
                        }
                        // Any tap (not a dismiss) brings the app forward, even if
                        // the deep link is missing (matching the prior behaviour):
                        // with no link there is no workspace to target, so that
                        // case routes to the main window.
                        let wake = crate::app_window::route_native_tap(app, owner.as_deref());
                        // Only an ALREADY-LOADED page gets the warm signal. The
                        // other targets are a page about to load, whose startup
                        // drain is the trigger, and an emit into a webview
                        // mid-navigation is dropped.
                        if let (Some(label), Some(link)) = (wake, link) {
                            let _ = app.emit_to(label, "native-notification-tapped", link);
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
    /// tap, plus the `workspace` (gateway slug) the calling page is served under.
    /// No-op in dev (see [`setup`]). Those two fields compose the request
    /// identifier ([`super::notification_identifier`]), which keys the
    /// pending-link map and scopes replace-by-id (the web-push `tag` equivalent)
    /// to the raising workspace.
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
    /// removes every delivered banner **`workspace` raised** (the mark-all-read
    /// path). Also drops the stashed deep link(s) from [`pending`] so a tap on a
    /// now-removed banner can't route. No-op in dev (see [`setup`]). An empty
    /// `Some("")` is a malformed single id → no-op (never an accidental
    /// dismiss-all), matching [`show`]'s empty-identifier handling.
    ///
    /// **Both arms are workspace-scoped, and both need to be.** `Some(id)`
    /// rebuilds the same composite identifier [`show`] posted, or it would match
    /// nothing. (Which is also why a banner delivered by a build older than the
    /// composite outlives BOTH arms: its identifier is bare, and every rebuilt
    /// one is composite. Transitional and self-healing, the same tolerance
    /// [`super::identifier_belongs_to`] documents for the sweep.) The dismiss-all
    /// arm used to call `removeAllDeliveredNotifications`,
    /// which wiped every OTHER workspace's banners on one workspace's
    /// mark-all-read; it now enumerates delivered banners and removes only the
    /// ones this workspace owns. The enumeration (rather than reading [`pending`],
    /// which holds only what THIS process showed) is what keeps a dismiss-all
    /// working across a client relaunch.
    ///
    /// UN's remove methods are safe to call from any thread (like
    /// `addNotificationRequest` in [`show`]), so no main-thread marshaling.
    pub fn dismiss(workspace: Option<String>, id: Option<String>) {
        if tauri::is_dev() {
            return;
        }
        let workspace = workspace.unwrap_or_default();
        let center = UNUserNotificationCenter::currentNotificationCenter();
        match id {
            Some(notification_id) => {
                let identifier = super::notification_identifier(&workspace, &notification_id);
                // Empty id: nothing actionable, and NEVER a fall-through to all.
                if identifier.is_empty() {
                    return;
                }
                let ids = NSArray::from_retained_slice(&[NSString::from_str(&identifier)]);
                center.removeDeliveredNotificationsWithIdentifiers(&ids);
                pending().lock().unwrap().remove(&identifier);
            }
            None => dismiss_all_for_workspace(&center, workspace),
        }
    }

    /// Remove every delivered banner belonging to `workspace`, leaving other
    /// workspaces' banners on screen. Backs the dismiss-all arm of [`dismiss`].
    ///
    /// [`pending`] is pruned synchronously (it is this process's own record, and
    /// a phantom tap on a banner we are about to remove must not route), then the
    /// delivered set is enumerated so banners from an earlier client process are
    /// swept too.
    fn dismiss_all_for_workspace(center: &UNUserNotificationCenter, workspace: String) {
        pending()
            .lock()
            .unwrap()
            .retain(|identifier, _| !super::identifier_belongs_to(identifier, &workspace));

        let handler = RcBlock::new(move |delivered: NonNull<NSArray<UNNotification>>| {
            // SAFETY: UN hands the completion handler a live, non-null array that
            // is valid for the duration of the call.
            let delivered = unsafe { delivered.as_ref() };
            let ids: Vec<Retained<NSString>> = delivered
                .iter()
                .map(|notification| notification.request().identifier())
                .filter(|identifier| {
                    super::identifier_belongs_to(&identifier.to_string(), &workspace)
                })
                .collect();
            if ids.is_empty() {
                return;
            }
            UNUserNotificationCenter::currentNotificationCenter()
                .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
        });
        center.getDeliveredNotificationsWithCompletionHandler(&handler);
        // Escaping completion handler, leaked for the same reason as `setup`'s
        // authorization block: UN copies it per ObjC convention, but that cannot
        // be runtime-tested here, so we guarantee it outlives the async callback
        // under any copy semantics. One tiny one-shot block per mark-all-read,
        // which is a rare user action.
        std::mem::forget(handler);
    }

    /// Set the dock-icon badge to `label`, or clear it with `None`. The caller
    /// (`crate::activation::apply_unread_indicator`) formats the AGGREGATE unread total across
    /// running workspaces (the Tauri window fronts the gateway, so its app icon
    /// represents all workspaces) — including the `0`→clear and `>99`→"99+" rules —
    /// and sends it here only while the client is a normal `Regular` Dock app; a
    /// menu-bar-only client has no Dock tile, so it gets `None`. The same count
    /// goes to [`set_tray_title`] either way, so the menu bar always shows it.
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

    /// Set the menu-bar tray icon's title text, clearing it with `""`. This is
    /// where the unread count lives at ALL times: `crate::activation::apply_unread_indicator`
    /// sends it here on every recompute, whatever the activation policy, so the
    /// menu bar is a constant read on how much is waiting. While a window is open
    /// the same count is on the Dock badge too ([`set_dock_badge`]); menu-bar-only
    /// has no Dock tile, so the tray is then the only surface. Looks up the tray by
    /// the id it was built with (`lucidos-tray`). Best-effort: a missing tray /
    /// failure is logged, not fatal.
    ///
    /// **`Some(title)` always, never `None`, and that is load-bearing.**
    /// `tray-icon`'s macOS backend clears nothing on `None`: `set_title_inner`
    /// wraps its `setTitle:` call in `if let Some(..)` and falls off the end
    /// otherwise, so the status item keeps whatever text it last received while
    /// the crate's own cached `attrs.title` records the clear. An empty string
    /// goes down the same path every real count does, blanking the button and
    /// letting `update_dimensions` shrink the item back to icon width. Passing
    /// `None` here is what froze the menu bar at a stale unread count while the
    /// bell and the Dock tile both read zero.
    pub fn set_tray_title(app: &AppHandle, title: &str) {
        if let Some(tray) = app.tray_by_id("lucidos-tray") {
            if let Err(e) = tray.set_title(Some(title)) {
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

    /// Drain the deep links stashed by taps the page serving `workspace` may not
    /// have been listening for. The frontend calls this on startup (cold path)
    /// AND on each `native-notification-tapped` signal (warm path); the Mutex
    /// makes the drain atomic so each tap routes exactly once across both paths
    /// and can't re-fire on a later reload. Naturally empty in dev (the delegate
    /// never fires) and once every tap has been consumed.
    ///
    /// **Scoped to the calling page's workspace** ([`super::tap_belongs_to`]).
    /// The stash is process-global while every window can sit on its own
    /// workspace, so an unconditional take let whichever page woke first swallow
    /// a tap raised by a workspace it is not serving. Another workspace's tap is
    /// LEFT IN PLACE for the window this tap's router is bringing up. See
    /// `native-push.ts` and `system-knowhow/notifications.md` §4.
    pub fn take_pending_taps(workspace: Option<&str>) -> Vec<serde_json::Value> {
        let mut taps = pending_taps().lock().unwrap();
        let (mine, theirs) = std::mem::take(&mut *taps)
            .into_iter()
            .partition(|link| super::tap_belongs_to(link, workspace));
        *taps = theirs;
        mine
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
pub fn dismiss(_workspace: Option<String>, _id: Option<String>) {}

/// No dock badge off macOS (the native app-icon badge is a macOS dock-tile
/// concept). Browser / PWA clients still get the Badging API on every platform.
#[cfg(not(target_os = "macos"))]
pub fn set_dock_badge(_label: Option<String>) {}

/// No tray-title unread count off macOS: the menu-bar status item is a macOS
/// concept. A no-op so `crate::activation::apply_unread_indicator` stays platform-agnostic.
#[cfg(not(target_os = "macos"))]
pub fn set_tray_title(_app: &tauri::AppHandle, _title: &str) {}

/// No native tap stash off macOS (no native notification path here); always
/// empty so the Tauri command stays platform-agnostic.
#[cfg(not(target_os = "macos"))]
pub fn take_pending_taps(_workspace: Option<&str>) -> Vec<serde_json::Value> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn link_identifier_composes_the_workspace_and_the_notification_id() {
        // Both present → the composite IS the request identifier, so a repeat
        // from the same workspace replaces its banner and two workspaces never
        // collide.
        assert_eq!(
            link_identifier(
                &json!({"workspace": "myws", "notification_id": "abc-123", "thread_id": "t1"})
            ),
            "myws|abc-123"
        );
        // No workspace (legacy direct engine, no gateway) → the bare id, exactly
        // as before this field existed.
        assert_eq!(
            link_identifier(&json!({"notification_id": "abc-123"})),
            "abc-123"
        );
        // Absent id → empty (the caller then skips the pending-link insert),
        // whatever the workspace says.
        assert_eq!(link_identifier(&json!({"thread_id": "t1"})), "");
        assert_eq!(link_identifier(&json!({"workspace": "myws"})), "");
        // Non-string → empty (defensive: a number/null is not a usable id).
        assert_eq!(link_identifier(&json!({"notification_id": 42})), "");
        assert_eq!(link_identifier(&json!({"notification_id": null})), "");
        assert_eq!(link_identifier(&serde_json::Value::Null), "");
    }

    #[test]
    fn notification_identifier_round_trips_through_split_identifier() {
        // The property the relaunch fallback depends on: whatever `show` posted,
        // a tap can recover from the delivered notification's identifier alone.
        for (workspace, id) in [("myws", "abc-123"), ("", "abc-123")] {
            let identifier = notification_identifier(workspace, id);
            let (parsed_ws, parsed_id) = split_identifier(&identifier);
            assert_eq!(parsed_ws, (!workspace.is_empty()).then_some(workspace));
            assert_eq!(parsed_id, id);
        }
    }

    #[test]
    fn split_identifier_reads_a_bare_identifier_as_workspaceless() {
        // A banner from a build older than the composite, or a no-gateway engine.
        assert_eq!(split_identifier("abc-123"), (None, "abc-123"));
        // Defensive: a leading separator is not a workspace named "".
        assert_eq!(split_identifier("|abc-123"), (None, "abc-123"));
        // First separator wins, so a stray one inside an id keeps the workspace.
        assert_eq!(split_identifier("myws|a|b"), (Some("myws"), "a|b"));
        assert_eq!(split_identifier(""), (None, ""));
    }

    #[test]
    fn identifier_belongs_to_scopes_the_dismiss_all() {
        assert!(identifier_belongs_to("myws|abc-123", "myws"));
        // The bug this fixes: another workspace's banner must survive a
        // mark-all-read here.
        assert!(!identifier_belongs_to("otherws|abc-123", "myws"));
        // A bare identifier belongs only to the no-gateway single workspace, so a
        // pre-composite leftover is never swept by a named workspace.
        assert!(identifier_belongs_to("abc-123", ""));
        assert!(!identifier_belongs_to("abc-123", "myws"));
        assert!(!identifier_belongs_to("myws|abc-123", ""));
    }

    /// `(label, url)` pairs in the shape `choose_tap_target` takes.
    fn windows(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(label, url)| ((*label).to_string(), (*url).to_string()))
            .collect()
    }

    const ORIGIN: &str = "http://localhost:3210";

    #[test]
    fn tap_belongs_to_leaves_another_workspaces_tap_in_the_stash() {
        let mine = json!({"notification_id": "n-1", "workspace": "myws"});
        let theirs = json!({"notification_id": "n-2", "workspace": "otherws"});
        // Unattributable: a legacy direct engine, or a pre-stamp banner. Nothing
        // to route it by, so any page may take it (the behaviour before the stamp).
        let unstamped = json!({"notification_id": "n-3"});

        assert!(tap_belongs_to(&mine, Some("myws")));
        // The bug this closes: whichever window drained first used to consume
        // every tap, including one raised by a workspace it is not serving.
        assert!(!tap_belongs_to(&theirs, Some("myws")));
        assert!(tap_belongs_to(&unstamped, Some("myws")));

        // A page with no workspace (legacy root engine) takes only unstamped taps.
        assert!(tap_belongs_to(&unstamped, None));
        assert!(!tap_belongs_to(&mine, None));

        // A null / non-string workspace reads as absent, not as a workspace named "".
        assert!(tap_belongs_to(&json!({"workspace": null}), Some("myws")));
        assert!(tap_belongs_to(&json!({"workspace": 42}), Some("myws")));
    }

    #[test]
    fn gateway_origin_is_the_first_window_actually_served_off_one() {
        // An unnavigated window contributes NO origin, so it is skipped rather
        // than yielding the literal "null" that nothing can parse. See
        // `window_target::window_origin`, which owns that rule and its cases.
        assert_eq!(
            gateway_origin(&windows(&[("main", "tauri://localhost")])),
            None
        );
        assert_eq!(gateway_origin(&windows(&[("main", "")])), None);
        assert_eq!(gateway_origin(&[]), None);

        assert_eq!(
            gateway_origin(&windows(&[
                ("main", "tauri://localhost"),
                ("window-1", "http://localhost:3210/myws/#thread=t1"),
            ])),
            Some("http://localhost:3210")
        );
    }

    #[test]
    fn a_tap_focuses_the_window_already_on_its_workspace() {
        // The whole point: `main` is on another workspace, so the OLD behaviour
        // (always show `main`) would have fronted the wrong window and then let
        // that page navigate itself away from what the user had open.
        let open = windows(&[
            ("main", "http://localhost:3210/otherws/"),
            ("window-1", "http://localhost:3210/myws/#thread=t1"),
        ]);
        assert_eq!(
            choose_tap_target(&open, Some("myws"), ORIGIN),
            TapTarget::Focus("window-1".to_string())
        );
    }

    #[test]
    fn main_wins_when_several_windows_are_on_the_raising_workspace() {
        let open = windows(&[
            ("window-2", "http://localhost:3210/myws/"),
            ("main", "http://localhost:3210/myws/"),
            ("window-1", "http://localhost:3210/myws/"),
        ]);
        assert_eq!(
            choose_tap_target(&open, Some("myws"), ORIGIN),
            TapTarget::Focus("main".to_string())
        );
        // Without `main`, the choice is the lowest label rather than whatever
        // order the window map happened to yield.
        let open = windows(&[
            ("window-2", "http://localhost:3210/myws/"),
            ("window-1", "http://localhost:3210/myws/"),
        ]);
        assert_eq!(
            choose_tap_target(&open, Some("myws"), ORIGIN),
            TapTarget::Focus("window-1".to_string())
        );
    }

    #[test]
    fn a_picker_window_is_pointed_at_the_raising_workspace() {
        // The trayed login-agent client: a hidden `main` parked on the picker.
        // Reusing it is what keeps a tap from opening a window every time.
        let open = windows(&[("main", "http://localhost:3210/~/")]);
        assert_eq!(
            choose_tap_target(&open, Some("myws"), ORIGIN),
            TapTarget::Navigate {
                label: "main".to_string(),
                url: "http://localhost:3210/myws/".to_string(),
            }
        );
    }

    #[test]
    fn a_tap_opens_a_window_rather_than_taking_one_off_another_workspace() {
        // Every window is on some other workspace. The old page-side hop yanked
        // one of them to the raising workspace; this must open a fresh window and
        // leave both of these alone.
        let open = windows(&[
            ("main", "http://localhost:3210/otherws/"),
            ("window-1", "http://localhost:3210/thirdws/"),
        ]);
        assert_eq!(
            choose_tap_target(&open, Some("myws"), ORIGIN),
            TapTarget::NewWindow {
                url: "http://localhost:3210/myws/".to_string(),
            }
        );
        // And with no windows at all (every one closed), same answer.
        assert_eq!(
            choose_tap_target(&[], Some("myws"), ORIGIN),
            TapTarget::NewWindow {
                url: "http://localhost:3210/myws/".to_string(),
            }
        );
    }

    #[test]
    fn a_still_booting_client_aims_its_first_navigation_at_the_workspace() {
        // `desktop::launch` is waiting on the gateway and will navigate `main`
        // itself, so pointing any window at the workspace here would be clobbered.
        let open = windows(&[("main", "tauri://localhost")]);
        assert_eq!(
            choose_tap_target(&open, Some("myws"), ORIGIN),
            TapTarget::LaunchInto {
                url: "http://localhost:3210/myws/".to_string(),
            }
        );
        // A window ALREADY on the workspace still wins over the boot branch.
        let open = windows(&[
            ("main", "tauri://localhost"),
            ("window-1", "http://localhost:3210/myws/"),
        ]);
        assert_eq!(
            choose_tap_target(&open, Some("myws"), ORIGIN),
            TapTarget::Focus("window-1".to_string())
        );
    }

    #[test]
    fn a_tap_with_no_usable_workspace_falls_back_to_the_main_window() {
        let open = windows(&[("main", "http://localhost:3210/otherws/")]);
        // Legacy direct engine / pre-stamp banner: nothing to target by.
        assert_eq!(
            choose_tap_target(&open, None, ORIGIN),
            TapTarget::MainWindow
        );
        // A malformed slug is never used to build a URL to open.
        assert_eq!(
            choose_tap_target(&open, Some("Not A Slug"), ORIGIN),
            TapTarget::MainWindow
        );
        assert_eq!(
            choose_tap_target(&open, Some(""), ORIGIN),
            TapTarget::MainWindow
        );
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
