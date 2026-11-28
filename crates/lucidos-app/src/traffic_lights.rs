use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[cfg(any(target_os = "macos", test))]
pub(crate) const LIGHTS_X_PX: f64 = 10.0;

const DEFAULT_BAR_HEIGHT_PX: f64 = 48.0;

const MIN_BAR_HEIGHT_PX: f64 = 16.0;
const MAX_BAR_HEIGHT_PX: f64 = 400.0;

static SEED_BAR_HEIGHT: AtomicU64 = AtomicU64::new(0);

static BAR_HEIGHTS: Mutex<BTreeMap<String, f64>> = Mutex::new(BTreeMap::new());

const BAR_HEIGHT_FILE: &str = "titlebar-bar-height";

fn is_plausible_bar_height(px: f64) -> bool {
    px.is_finite() && (MIN_BAR_HEIGHT_PX..=MAX_BAR_HEIGHT_PX).contains(&px)
}

#[cfg(any(target_os = "macos", test))]
fn container_height(bar_height_px: f64, button_origin_y: f64, button_height: f64) -> f64 {
    bar_height_px / 2.0 + button_origin_y + button_height / 2.0
}

fn bar_height_or_default(persisted: Option<&str>) -> f64 {
    persisted
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|px| is_plausible_bar_height(*px))
        .unwrap_or(DEFAULT_BAR_HEIGHT_PX)
}

fn format_bar_height(px: f64) -> String {
    format!("{px}")
}

fn persist_bar_height(app: &tauri::AppHandle, px: f64) {
    crate::config_scalar::write_if_changed(
        app,
        BAR_HEIGHT_FILE,
        &format_bar_height(px),
        "header-bar height",
    );
}

#[cfg(all(target_os = "macos", feature = "window-probe"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClusterGeometry {
    pub(crate) centre_from_top: f64,
    pub(crate) left_x: f64,
}

#[cfg(all(target_os = "macos", feature = "window-probe"))]
pub(crate) fn measure_cluster(ns_window: &objc2_app_kit::NSWindow) -> Option<ClusterGeometry> {
    use objc2_app_kit::NSWindowButton;

    let close = ns_window.standardWindowButton(NSWindowButton::CloseButton)?;
    let container = titlebar_container(&close)?;
    let frame = close.frame();
    let centre_in_window = container.frame().origin.y + frame.origin.y + frame.size.height / 2.0;
    Some(ClusterGeometry {
        centre_from_top: ns_window.frame().size.height - centre_in_window,
        left_x: frame.origin.x,
    })
}

fn bar_height_for(label: &str) -> f64 {
    if let Some(px) = BAR_HEIGHTS.lock().unwrap().get(label).copied() {
        return px;
    }
    let seed = f64::from_bits(SEED_BAR_HEIGHT.load(Ordering::SeqCst));
    if is_plausible_bar_height(seed) {
        seed
    } else {
        DEFAULT_BAR_HEIGHT_PX
    }
}

pub(crate) fn load_persisted(app: &tauri::AppHandle) {
    let persisted = crate::config_scalar::path(app, BAR_HEIGHT_FILE)
        .as_deref()
        .and_then(crate::config_scalar::read);
    let px = bar_height_or_default(persisted.as_deref());
    SEED_BAR_HEIGHT.store(px.to_bits(), Ordering::SeqCst);
}

pub(crate) fn place(window: &tauri::Window) {
    place_at(window, bar_height_for(window.label()));
}

pub(crate) fn place_all(app: &tauri::AppHandle) {
    for (label, window) in tauri::Manager::windows(app) {
        if crate::app_window::is_app_window(&label) {
            place_at(&window, bar_height_for(&label));
        }
    }
}

pub(crate) fn set_bar_height(
    app: &tauri::AppHandle,
    window: &tauri::Window,
    bar_height_px: f64,
) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    if !is_plausible_bar_height(bar_height_px) {
        return Err(format!(
            "implausible header-bar height {bar_height_px}px (expected {MIN_BAR_HEIGHT_PX} to \
             {MAX_BAR_HEIGHT_PX})"
        ));
    }
    BAR_HEIGHTS
        .lock()
        .unwrap()
        .insert(window.label().to_string(), bar_height_px);
    SEED_BAR_HEIGHT.store(bar_height_px.to_bits(), Ordering::SeqCst);
    place_at(window, bar_height_px);
    persist_bar_height(app, bar_height_px);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn place_at(_window: &tauri::Window, _bar_height_px: f64) {}

#[cfg(target_os = "macos")]
fn place_at(window: &tauri::Window, bar_height_px: f64) {
    let label = window.label().to_string();
    let placed = on_ns_window(window, move |ns_window| {
        watch(&label, ns_window);
        inset_lights(ns_window, LIGHTS_X_PX, bar_height_px);
    });
    if let Err(e) = placed {
        eprintln!("[Tauri] Could not place the traffic lights: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn retitle(window: &tauri::Window, title: String) -> Result<(), String> {
    window.set_title(&title).map_err(|e| format!("{e}"))
}

#[cfg(target_os = "macos")]
pub(crate) fn retitle(window: &tauri::Window, title: String) -> Result<(), String> {
    let label = window.label().to_string();
    on_ns_window(window, move |ns_window| {
        retitle_and_place(ns_window, &title, bar_height_for(&label));
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn retitle_and_place(ns_window: &objc2_app_kit::NSWindow, title: &str, bar_px: f64) {
    ns_window.setTitle(&objc2_foundation::NSString::from_str(title));
    inset_lights(ns_window, LIGHTS_X_PX, bar_px);
}

#[cfg(target_os = "macos")]
fn on_ns_window(
    window: &tauri::Window,
    apply: impl FnOnce(&objc2_app_kit::NSWindow) + Send + 'static,
) -> Result<(), String> {
    let Some(_mtm) = objc2::MainThreadMarker::new() else {
        let deferred = window.clone();
        return window
            .run_on_main_thread(move || {
                if let Err(e) = on_ns_window(&deferred, apply) {
                    eprintln!("[Tauri] {e}");
                }
            })
            .map_err(|e| format!("could not reach the main thread: {e}"));
    };

    let ptr = window
        .ns_window()
        .map_err(|e| format!("no NSWindow for {}: {e}", window.label()))?;
    if ptr.is_null() {
        return Err(format!("no NSWindow for {}", window.label()));
    }
    // SAFETY: `ns_window` hands back an autoreleased `NSWindow` for this window,
    // valid for the rest of this call. `_mtm` is the evidence that forming a
    // reference to a `MainThreadOnly` type here is sound.
    apply(unsafe { &*ptr.cast() });
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) type Observer =
    objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>;

#[cfg(target_os = "macos")]
struct Watch {
    resize: Observer,
    relayout: Observer,
    container: objc2::rc::Retained<objc2_app_kit::NSView>,
}

#[cfg(target_os = "macos")]
thread_local! {
    static WATCHES: std::cell::RefCell<std::collections::HashMap<String, Watch>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

#[cfg(target_os = "macos")]
fn watch(label: &str, ns_window: &objc2_app_kit::NSWindow) {
    use objc2_app_kit::NSWindowButton;

    if !crate::app_window::is_app_window(label) {
        return;
    }
    let Some(container) = ns_window
        .standardWindowButton(NSWindowButton::CloseButton)
        .and_then(|close| titlebar_container(&close))
    else {
        return;
    };
    let watched = WATCHES.with_borrow(|watches| {
        watches.get(label).is_some_and(|watch| {
            objc2::rc::Retained::as_ptr(&watch.container) == objc2::rc::Retained::as_ptr(&container)
        })
    });
    if watched {
        return;
    }
    remove_watch(label);

    let owner = label.to_string();
    let resize = observe_resizes(ns_window, move |window| {
        inset_lights(window, LIGHTS_X_PX, bar_height_for(&owner));
    });
    let owner = label.to_string();
    let relayout = observe_relayouts(ns_window, &container, move |window| {
        inset_lights(window, LIGHTS_X_PX, bar_height_for(&owner));
    });
    let watch = Watch {
        resize,
        relayout,
        container,
    };
    WATCHES.with_borrow_mut(|watches| watches.insert(label.to_string(), watch));
}

#[cfg(target_os = "macos")]
pub(crate) fn observe_resizes(
    ns_window: &objc2_app_kit::NSWindow,
    place: impl Fn(&objc2_app_kit::NSWindow) + 'static,
) -> Observer {
    let object: &objc2::runtime::AnyObject = ns_window;
    // SAFETY: AppKit's own notification name constant, never written.
    let name = unsafe { objc2_app_kit::NSWindowDidResizeNotification };
    observe(name, object, move |object| {
        // SAFETY: the observer is scoped to this one window through
        // `object:`, and it is alive because it is the one posting.
        let ns_window: &objc2_app_kit::NSWindow =
            unsafe { &*(object as *const objc2::runtime::AnyObject).cast() };
        place(ns_window);
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn observe_relayouts(
    ns_window: &objc2_app_kit::NSWindow,
    container: &objc2_app_kit::NSView,
    place: impl Fn(&objc2_app_kit::NSWindow) + 'static,
) -> Observer {
    let owner = objc2::rc::Weak::from(ns_window);
    let queued = std::rc::Rc::new(std::cell::Cell::new(false));
    let deferred = block2::RcBlock::new({
        let queued = queued.clone();
        move || {
            queued.set(false);
            let Some(ns_window) = owner.load() else {
                return;
            };
            if ns_window
                .styleMask()
                .contains(objc2_app_kit::NSWindowStyleMask::FullScreen)
            {
                return;
            }
            place(&ns_window);
        }
    });
    let object: &objc2::runtime::AnyObject = container;
    // SAFETY: AppKit's own notification name constant, never written.
    let name = unsafe { objc2_app_kit::NSViewFrameDidChangeNotification };
    observe(name, object, move |_container| {
        if queued.replace(true) {
            return;
        }
        // SAFETY: the binding asks for a sendable block because a queue may
        // run it on any thread. The main queue runs it on the main thread,
        // which is where this block was made and where its captures belong.
        unsafe { objc2_foundation::NSOperationQueue::mainQueue().addOperationWithBlock(&deferred) };
    })
}

#[cfg(target_os = "macos")]
fn observe(
    name: &objc2_foundation::NSNotificationName,
    object: &objc2::runtime::AnyObject,
    handle: impl Fn(&objc2::runtime::AnyObject) + 'static,
) -> Observer {
    use objc2_foundation::{NSNotification, NSNotificationCenter};

    let block = block2::RcBlock::new(move |notification: std::ptr::NonNull<NSNotification>| {
        if objc2::MainThreadMarker::new().is_none() {
            return;
        }
        // SAFETY: the notification is alive for the duration of the call.
        if let Some(object) = (unsafe { notification.as_ref() }).object() {
            handle(&object);
        }
    });
    // SAFETY: the name is AppKit's own notification constant, and scoping to
    // `object` means only that sender can reach the block.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            Some(object),
            None,
            &block,
        )
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn stop_observing(observer: &Observer) {
    let observer: &objc2::runtime::AnyObject = observer.as_ref();
    // SAFETY: the token is what `addObserverForName:object:queue:usingBlock:`
    // handed back, on the same centre.
    unsafe { objc2_foundation::NSNotificationCenter::defaultCenter().removeObserver(observer) };
}

#[cfg(target_os = "macos")]
fn remove_watch(label: &str) {
    if let Some(watch) = WATCHES.with_borrow_mut(|watches| watches.remove(label)) {
        stop_observing(&watch.resize);
        stop_observing(&watch.relayout);
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn unwatch(label: &str) {
    BAR_HEIGHTS.lock().unwrap().remove(label);
}

#[cfg(target_os = "macos")]
pub(crate) fn unwatch(label: &str) {
    BAR_HEIGHTS.lock().unwrap().remove(label);
    if objc2::MainThreadMarker::new().is_none() {
        eprintln!(
            "[Tauri] Traffic-light observers for {label} left registered: not on the main thread"
        );
        return;
    }
    remove_watch(label);
}

#[cfg(target_os = "macos")]
pub(crate) fn titlebar_container(
    close: &objc2_app_kit::NSButton,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    // SAFETY: `superview` is unsafe in the generated bindings because it is
    // unbounded in what it can return. We only read and write frames on the
    // result, and holding a button is evidence we are on the main thread.
    unsafe { close.superview().and_then(|view| view.superview()) }
}

#[cfg(target_os = "macos")]
pub(crate) fn inset_lights(ns_window: &objc2_app_kit::NSWindow, x: f64, bar_height_px: f64) {
    use objc2_app_kit::NSWindowButton;

    let (Some(close), Some(miniaturize)) = (
        ns_window.standardWindowButton(NSWindowButton::CloseButton),
        ns_window.standardWindowButton(NSWindowButton::MiniaturizeButton),
    ) else {
        return;
    };
    let zoom = ns_window.standardWindowButton(NSWindowButton::ZoomButton);
    let Some(container) = titlebar_container(&close) else {
        return;
    };

    let close_frame = close.frame();
    let height = container_height(bar_height_px, close_frame.origin.y, close_frame.size.height);
    let mut container_frame = container.frame();
    container_frame.size.height = height;
    container_frame.origin.y = ns_window.frame().size.height - height;
    if container.frame() != container_frame {
        container.setFrame(container_frame);
    }

    let pitch = miniaturize.frame().origin.x - close_frame.origin.x;
    let mut buttons = vec![close, miniaturize];
    buttons.extend(zoom);
    for (index, button) in buttons.into_iter().enumerate() {
        let mut origin = button.frame().origin;
        origin.x = x + index as f64 * pitch;
        if button.frame().origin != origin {
            button.setFrameOrigin(origin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_48px_bar_centres_the_cluster_24px_down() {
        assert_eq!(container_height(48.0, 9.0, 14.0), 40.0);
    }

    #[test]
    fn the_cluster_centres_on_the_bar_at_every_supported_scale() {
        for bar in [36.0, 48.0, 54.0, 60.0, 66.0, 72.0, 84.0, 96.0_f64] {
            let height = container_height(bar, 9.0, 14.0);
            let centre_from_top = height - 9.0 - 7.0;
            assert_eq!(centre_from_top, bar / 2.0, "bar {bar}");
        }
    }

    #[test]
    fn the_arithmetic_follows_appkit_rather_than_the_measured_9_and_14() {
        let height = container_height(48.0, 11.0, 16.0);
        assert_eq!(height - 11.0 - 8.0, 24.0);
    }

    #[test]
    fn a_missing_or_unreadable_file_falls_back_to_the_compiled_default() {
        assert_eq!(bar_height_or_default(None), DEFAULT_BAR_HEIGHT_PX);
        assert_eq!(bar_height_or_default(Some("")), DEFAULT_BAR_HEIGHT_PX);
        assert_eq!(bar_height_or_default(Some("  ")), DEFAULT_BAR_HEIGHT_PX);
        assert_eq!(
            bar_height_or_default(Some("forty-eight")),
            DEFAULT_BAR_HEIGHT_PX
        );
        assert_eq!(bar_height_or_default(Some("4")), DEFAULT_BAR_HEIGHT_PX);
    }

    #[test]
    fn an_implausible_persisted_value_degrades_to_the_default() {
        for raw in ["0", "-48", "1e9", "NaN", "inf"] {
            assert_eq!(
                bar_height_or_default(Some(raw)),
                DEFAULT_BAR_HEIGHT_PX,
                "{raw}"
            );
        }
    }

    #[test]
    fn a_persisted_value_round_trips_through_the_written_form() {
        for px in [36.0, 48.0, 54.0, 66.0, 72.0, 96.0_f64] {
            let written = format_bar_height(px);
            assert_eq!(bar_height_or_default(Some(&written)), px, "{px}");
            let padded = format!(" {written}\n");
            assert_eq!(bar_height_or_default(Some(padded.trim())), px, "{px}");
        }
    }

    #[test]
    fn the_written_form_is_already_trimmed() {
        let written = format_bar_height(48.0);
        assert_eq!(written.trim(), written);
        assert_eq!(written, "48");
    }

    #[test]
    fn a_persisted_bar_height_round_trips_through_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BAR_HEIGHT_FILE);

        assert_eq!(crate::config_scalar::read(&path), None);
        assert_eq!(bar_height_or_default(None), DEFAULT_BAR_HEIGHT_PX);

        std::fs::write(&path, format_bar_height(72.0)).unwrap();
        let read = crate::config_scalar::read(&path);
        assert_eq!(read.as_deref(), Some("72"));
        assert_eq!(bar_height_or_default(read.as_deref()), 72.0);
    }

    static GLOBALS: Mutex<()> = Mutex::new(());

    fn clear_globals() {
        BAR_HEIGHTS.lock().unwrap().clear();
        SEED_BAR_HEIGHT.store(0_f64.to_bits(), Ordering::SeqCst);
    }

    #[test]
    fn each_window_places_against_the_bar_its_own_page_reported() {
        let _held = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        clear_globals();
        let mut heights = BAR_HEIGHTS.lock().unwrap();
        heights.insert("main".to_string(), 48.0);
        heights.insert("window-1".to_string(), 54.0);
        drop(heights);

        assert_eq!(bar_height_for("main"), 48.0);
        assert_eq!(bar_height_for("window-1"), 54.0);
        assert_eq!(bar_height_for("window-2"), DEFAULT_BAR_HEIGHT_PX);
        SEED_BAR_HEIGHT.store(72_f64.to_bits(), Ordering::SeqCst);
        assert_eq!(bar_height_for("window-2"), 72.0);
        assert_eq!(bar_height_for("main"), 48.0, "a report beats the seed");
        clear_globals();
    }

    #[test]
    fn a_closed_window_takes_its_reported_bar_with_it() {
        let _held = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        clear_globals();
        BAR_HEIGHTS
            .lock()
            .unwrap()
            .insert("window-9".to_string(), 96.0);
        assert_eq!(bar_height_for("window-9"), 96.0);
        unwatch("window-9");
        assert_eq!(bar_height_for("window-9"), DEFAULT_BAR_HEIGHT_PX);
        clear_globals();
    }

    #[test]
    fn the_plausibility_bounds_admit_every_supported_ui_scale() {
        assert!(is_plausible_bar_height(36.0));
        assert!(is_plausible_bar_height(96.0));
        assert!(!is_plausible_bar_height(0.0));
        assert!(!is_plausible_bar_height(-1.0));
        assert!(!is_plausible_bar_height(f64::NAN));
        assert!(!is_plausible_bar_height(f64::INFINITY));
    }

    #[test]
    fn the_chosen_x_is_half_the_reserves_slack() {
        assert_eq!(LIGHTS_X_PX + 60.0 + LIGHTS_X_PX, 80.0);
    }
}
