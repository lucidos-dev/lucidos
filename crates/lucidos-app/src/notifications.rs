#[cfg(any(target_os = "macos", test))]
use crate::window_target::{
    is_workspace_route, preferred_label, window_context, windows_in, workspace_url, WindowContext,
};
use crate::window_target::{window_origin, LiveWindow};

#[cfg(any(target_os = "macos", test))]
const IDENTIFIER_SEPARATOR: char = '|';

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

#[cfg(any(target_os = "macos", test))]
fn split_identifier(identifier: &str) -> (Option<&str>, &str) {
    match identifier.split_once(IDENTIFIER_SEPARATOR) {
        Some((workspace, id)) if !workspace.is_empty() => (Some(workspace), id),
        Some((_, id)) => (None, id),
        None => (None, identifier),
    }
}

#[cfg(any(target_os = "macos", test))]
fn identifier_belongs_to(identifier: &str, workspace: &str) -> bool {
    match split_identifier(identifier).0 {
        Some(owner) => owner == workspace,
        None => workspace.is_empty(),
    }
}

#[cfg(any(target_os = "macos", test))]
fn link_field<'a>(link: &'a serde_json::Value, key: &str) -> &'a str {
    link.get(key).and_then(|v| v.as_str()).unwrap_or_default()
}

#[cfg(any(target_os = "macos", test))]
fn link_identifier(link: &serde_json::Value) -> String {
    notification_identifier(
        link_field(link, "workspace"),
        link_field(link, "notification_id"),
    )
}

#[cfg(any(target_os = "macos", test))]
fn is_dismiss_action(action: &str) -> bool {
    action.ends_with("DismissActionIdentifier")
}

#[cfg(any(target_os = "macos", test))]
fn link_workspace(link: &serde_json::Value) -> Option<&str> {
    let workspace = link_field(link, "workspace");
    (!workspace.is_empty()).then_some(workspace)
}

#[cfg(any(target_os = "macos", test))]
fn tap_belongs_to(link: &serde_json::Value, workspace: Option<&str>) -> bool {
    match link_workspace(link) {
        None => true,
        Some(owner) => Some(owner) == workspace,
    }
}

pub(crate) fn gateway_origin(windows: &[LiveWindow]) -> Option<&str> {
    windows.iter().find_map(|window| window_origin(&window.url))
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TapTarget {
    Focus(String),
    Navigate { label: String, url: String },
    NewWindow { url: String },
    LaunchInto { url: String },
    MainWindow,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn choose_tap_target(
    windows: &[LiveWindow],
    owner: Option<&str>,
    origin: &str,
) -> TapTarget {
    let Some(owner) = owner.filter(|o| is_workspace_route(o)) else {
        return TapTarget::MainWindow;
    };
    let url = workspace_url(origin, owner);

    if let Some(label) = preferred_label(&windows_in(windows, WindowContext::Workspace(owner))) {
        return TapTarget::Focus(label.to_string());
    }
    if windows.iter().any(|window| {
        window.label == crate::app_window::MAIN_WINDOW_LABEL
            && window_context(&window.url) == WindowContext::Unnavigated
    }) {
        return TapTarget::LaunchInto { url };
    }
    if let Some(label) = preferred_label(&windows_in(windows, WindowContext::Neutral)) {
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

    static APP: OnceLock<AppHandle> = OnceLock::new();

    const MAX_PENDING: usize = 256;

    fn pending() -> &'static Mutex<HashMap<String, serde_json::Value>> {
        static P: OnceLock<Mutex<HashMap<String, serde_json::Value>>> = OnceLock::new();
        P.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn pending_taps() -> &'static Mutex<Vec<serde_json::Value>> {
        static T: OnceLock<Mutex<Vec<serde_json::Value>>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(Vec::new()))
    }

    define_class!(
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
                let action = response.actionIdentifier().to_string();
                let routed = !super::is_dismiss_action(&action);

                let link = pending().lock().unwrap().remove(&identifier).or_else(|| {
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
                        let wake = crate::app_window::route_native_tap(app, owner.as_deref());
                        if let (Some(label), Some(link)) = (wake, link) {
                            let _ = app.emit_to(label, "native-notification-tapped", link);
                        }
                    }
                }

                completion_handler.call(());
            }

            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion_handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }
        }
    );

    fn new_delegate() -> Retained<Delegate> {
        unsafe { msg_send![Delegate::alloc(), init] }
    }

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
        std::mem::forget(handler);
    }

    pub fn show(title: &str, body: &str, link: serde_json::Value) -> Result<bool, String> {
        if tauri::is_dev() {
            return Ok(false);
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
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let completion = RcBlock::new(move |error: *mut NSError| {
            let result = if error.is_null() {
                Ok(true)
            } else {
                Err(unsafe { (&*error).localizedDescription().to_string() })
            };
            let _ = sender.send(result);
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&completion));
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| format!("Native notification acceptance timed out: {error}"))?
    }

    pub fn dismiss(workspace: Option<String>, id: Option<String>) {
        if tauri::is_dev() {
            return;
        }
        let workspace = workspace.unwrap_or_default();
        let center = UNUserNotificationCenter::currentNotificationCenter();
        match id {
            Some(notification_id) => {
                let identifier = super::notification_identifier(&workspace, &notification_id);
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
        std::mem::forget(handler);
    }

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

    pub fn set_tray_title(app: &AppHandle, title: &str) {
        if let Some(tray) = app.tray_by_id("lucidos-tray") {
            if let Err(e) = tray.set_title(Some(title)) {
                eprintln!("[Tauri] Failed to set tray title: {e}");
            }
        }
    }

    pub fn activate_app() {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }

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

#[cfg(not(target_os = "macos"))]
pub fn setup(_app: &tauri::AppHandle) {}

#[cfg(not(target_os = "macos"))]
pub fn show(_title: &str, _body: &str, _link: serde_json::Value) -> Result<bool, String> {
    Err("Native notifications are unavailable on this platform".into())
}

#[cfg(not(target_os = "macos"))]
pub fn dismiss(_workspace: Option<String>, _id: Option<String>) {}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_badge(_label: Option<String>) {}

#[cfg(not(target_os = "macos"))]
pub fn set_tray_title(_app: &tauri::AppHandle, _title: &str) {}

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
        assert_eq!(
            link_identifier(
                &json!({"workspace": "myws", "notification_id": "abc-123", "thread_id": "t1"})
            ),
            "myws|abc-123"
        );
        assert_eq!(
            link_identifier(&json!({"notification_id": "abc-123"})),
            "abc-123"
        );
        assert_eq!(link_identifier(&json!({"thread_id": "t1"})), "");
        assert_eq!(link_identifier(&json!({"workspace": "myws"})), "");
        assert_eq!(link_identifier(&json!({"notification_id": 42})), "");
        assert_eq!(link_identifier(&json!({"notification_id": null})), "");
        assert_eq!(link_identifier(&serde_json::Value::Null), "");
    }

    #[test]
    fn notification_identifier_round_trips_through_split_identifier() {
        for (workspace, id) in [("myws", "abc-123"), ("", "abc-123")] {
            let identifier = notification_identifier(workspace, id);
            let (parsed_ws, parsed_id) = split_identifier(&identifier);
            assert_eq!(parsed_ws, (!workspace.is_empty()).then_some(workspace));
            assert_eq!(parsed_id, id);
        }
    }

    #[test]
    fn split_identifier_reads_a_bare_identifier_as_workspaceless() {
        assert_eq!(split_identifier("abc-123"), (None, "abc-123"));
        assert_eq!(split_identifier("|abc-123"), (None, "abc-123"));
        assert_eq!(split_identifier("myws|a|b"), (Some("myws"), "a|b"));
        assert_eq!(split_identifier(""), (None, ""));
    }

    #[test]
    fn identifier_belongs_to_scopes_the_dismiss_all() {
        assert!(identifier_belongs_to("myws|abc-123", "myws"));
        assert!(!identifier_belongs_to("otherws|abc-123", "myws"));
        assert!(identifier_belongs_to("abc-123", ""));
        assert!(!identifier_belongs_to("abc-123", "myws"));
        assert!(!identifier_belongs_to("myws|abc-123", ""));
    }

    use crate::window_target::tests::{hidden, windows};

    const ORIGIN: &str = "http://localhost:3210";

    #[test]
    fn a_tap_fronts_the_visible_window_rather_than_a_hidden_main() {
        let open = hidden(
            windows(&[
                ("main", "http://localhost:3210/personal/"),
                ("window-2", "http://localhost:3210/personal/"),
            ]),
            "main",
        );
        assert_eq!(
            choose_tap_target(&open, Some("personal"), ORIGIN),
            TapTarget::Focus("window-2".to_string())
        );
    }

    #[test]
    fn tap_belongs_to_leaves_another_workspaces_tap_in_the_stash() {
        let mine = json!({"notification_id": "n-1", "workspace": "myws"});
        let theirs = json!({"notification_id": "n-2", "workspace": "otherws"});
        let unstamped = json!({"notification_id": "n-3"});

        assert!(tap_belongs_to(&mine, Some("myws")));
        assert!(!tap_belongs_to(&theirs, Some("myws")));
        assert!(tap_belongs_to(&unstamped, Some("myws")));

        assert!(tap_belongs_to(&unstamped, None));
        assert!(!tap_belongs_to(&mine, None));

        assert!(tap_belongs_to(&json!({"workspace": null}), Some("myws")));
        assert!(tap_belongs_to(&json!({"workspace": 42}), Some("myws")));
    }

    #[test]
    fn gateway_origin_is_the_first_window_actually_served_off_one() {
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
        assert_eq!(
            choose_tap_target(&[], Some("myws"), ORIGIN),
            TapTarget::NewWindow {
                url: "http://localhost:3210/myws/".to_string(),
            }
        );
    }

    #[test]
    fn a_tap_from_a_remote_workspace_lands_on_its_outside_route() {
        let open = windows(&[
            ("main", "http://localhost:3210/dev/"),
            ("window-1", "http://localhost:3210/dev@cloud/#thread=t1"),
        ]);
        assert_eq!(
            choose_tap_target(&open, Some("dev@cloud"), ORIGIN),
            TapTarget::Focus("window-1".to_string())
        );
        let open = windows(&[("main", "http://localhost:3210/dev/")]);
        assert_eq!(
            choose_tap_target(&open, Some("dev@cloud"), ORIGIN),
            TapTarget::NewWindow {
                url: "http://localhost:3210/dev@cloud/".to_string(),
            }
        );
        assert_eq!(
            split_identifier(&notification_identifier("dev@cloud", "n1")),
            (Some("dev@cloud"), "n1")
        );
    }

    #[test]
    fn a_still_booting_client_aims_its_first_navigation_at_the_workspace() {
        let open = windows(&[("main", "tauri://localhost")]);
        assert_eq!(
            choose_tap_target(&open, Some("myws"), ORIGIN),
            TapTarget::LaunchInto {
                url: "http://localhost:3210/myws/".to_string(),
            }
        );
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
        assert_eq!(
            choose_tap_target(&open, None, ORIGIN),
            TapTarget::MainWindow
        );
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
        assert!(is_dismiss_action(
            "com.apple.UNNotificationDismissActionIdentifier"
        ));
        assert!(!is_dismiss_action(
            "com.apple.UNNotificationDefaultActionIdentifier"
        ));
        assert!(!is_dismiss_action("my.custom.action"));
        assert!(!is_dismiss_action(""));
    }
}
