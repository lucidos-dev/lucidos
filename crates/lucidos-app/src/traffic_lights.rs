//! Where the macOS traffic lights sit. ADR 0074 records why we place them
//! ourselves and re-apply from AppKit's own resize notification.
//!
//! Under `titleBarStyle: "Overlay"` the webview owns the full window height and
//! the three window buttons float above it in an AppKit layer. That leaves two
//! numbers to own, and this module owns both: the x the cluster starts at
//! ([`LIGHTS_X_PX`]) and the y that centres the cluster on our header bar.
//!
//! **The geometry is measured, not assumed.** An AppKit probe against this
//! build's style mask read back:
//!
//!  * a 14pt button frame carrying the 12pt drawn circle
//!  * a 23pt pitch between buttons
//!  * a frame origin of (9, 9) inside a 32pt `NSTitlebarContainerView`
//!
//! So the cluster is 60pt wide, its centre 16pt below the window's top edge.
//! The buttons' `origin.y` is AppKit's to set, which is why
//! [`container_height`] is the arithmetic rather than a y offset. And AppKit
//! reverts the placement on **every window resize**, **every new title** and
//! **an appearance change**. [`watch`] and [`retitle`] own re-applying it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Where the LEFT edge of the traffic-light cluster goes, in logical px from
/// the window's left edge. Ours to choose, and one pixel off AppKit's own.
///
/// It is half of the slack in the 80px the header row keeps clear, the other
/// half being `--titlebar-lights-gap` in `styles/panels/shell.css`. 10 and 10
/// around a 60pt cluster is what centres the lights in that room: the 12pt
/// circle sits 1pt inside its 14pt frame, so the drawn cluster spans 11 to 69
/// with the same air on each side.
///
/// The single source for the number. [`crate::titlebar_inset_script`] stamps it
/// into `--titlebar-lights-x` before first paint, and the CSS derives
/// `--titlebar-lights-reserve` from it. The cluster's own 60pt width is NOT
/// here: it is a measured AppKit fact rather than a choice, this module reads
/// the pitch off the buttons instead, and CSS is the only consumer.
///
/// Gated the way `notifications.rs` gates its platform-free helpers. Nothing
/// off macOS has window buttons to place, and `test` keeps it available to the
/// unit tests, which run everywhere.
#[cfg(any(target_os = "macos", test))]
pub(crate) const LIGHTS_X_PX: f64 = 10.0;

/// The bar height to place against before any frontend has reported one: the
/// desktop bar at the default UI scale. The FALLBACK only, for a first run and
/// an unreadable file. A reported height is remembered in [`BAR_HEIGHT_FILE`],
/// so a user at 150% is not cold-launched into lights centred for 48px.
const DEFAULT_BAR_HEIGHT_PX: f64 = 48.0;

/// Bounds on a bar height we are willing to place lights against. The supported
/// UI-scale range puts the real bar between 36px and 96px, and these are
/// deliberately much wider: the Style Remote can retune the tokens the bar is
/// built from, and this is not the place to second-guess the frontend's own
/// measurement. They exist to reject a value that could only be a bug, such as
/// a zero, a negative, a NaN or a misplaced decimal point.
const MIN_BAR_HEIGHT_PX: f64 = 16.0;
const MAX_BAR_HEIGHT_PX: f64 = 400.0;

/// The bar height to place a window against before its own page has reported
/// one, as `f64::to_bits`. Loaded from disk in [`load_persisted`] and rewritten
/// by every frontend push. `0` means "nothing loaded yet" and is unambiguous,
/// since `0.0` is not a plausible bar height.
///
/// A GUESS, and it covers the frames between a window appearing and its page
/// measuring. Two surfaces disagree about the bar: the workspace shell renders
/// at the user's UI scale, and the picker at the browser default. So the last
/// pusher wins here, and each window's own report wins over it at once.
static SEED_BAR_HEIGHT: AtomicU64 = AtomicU64::new(0);

/// What each window's OWN page reported, keyed by window label.
///
/// Per window, because the bar is a property of the SURFACE a window is showing
/// rather than of the device. A single shared value let the picker's 48px bar
/// move a workspace window's lights, and the workspace's scaled bar move the
/// picker's.
static BAR_HEIGHTS: Mutex<BTreeMap<String, f64>> = Mutex::new(BTreeMap::new());

/// Remembers the last bar height the frontend reported, so a cold launch places
/// the lights on the user's bar rather than the compiled default. A bare
/// number, no schema: one value, read back through the same plausibility check
/// anything else is, and never trusted into anything but arithmetic.
///
/// Beside `config/titlebar-color`, `config/engine-port` and
/// `config/workspaces.json` in the app data dir, so a delete-data uninstall
/// (`desktop::support_data_paths`) forgets it along with everything else.
const BAR_HEIGHT_FILE: &str = "titlebar-bar-height";

/// True for a bar height worth placing lights against. Pure.
fn is_plausible_bar_height(px: f64) -> bool {
    px.is_finite() && (MIN_BAR_HEIGHT_PX..=MAX_BAR_HEIGHT_PX).contains(&px)
}

/// The height `NSTitlebarContainerView` must take for the cluster's vertical
/// centre to land `bar_height_px / 2` below the window's top edge, i.e. on the
/// centre of our own bar. Pure, so the one piece of arithmetic in this file is
/// testable without a window server.
///
/// The container is pinned to the window's top edge and AppKit keeps the
/// buttons vertically centred inside it, leaving their `origin.y` untouched. So
/// a button's centre sits `container_height - button_origin_y - button_height /
/// 2` below the window's top edge, and this solves that for the height.
///
/// Both AppKit terms are READ at the call site rather than baked in, so a macOS
/// release that retunes the titlebar keeps the cluster centred. The 14pt frame
/// and the 12pt circle share a centre, so centring the frame centres the light.
///
/// Gated like [`LIGHTS_X_PX`]: its only non-test caller is the macOS placement.
#[cfg(any(target_os = "macos", test))]
fn container_height(bar_height_px: f64, button_origin_y: f64, button_height: f64) -> f64 {
    bar_height_px / 2.0 + button_origin_y + button_height / 2.0
}

/// Pure: which bar height to place with, given the raw file content from the
/// last push. The persisted value wins only if it still parses and is still
/// plausible. A truncated or hand-edited file degrades to
/// [`DEFAULT_BAR_HEIGHT_PX`] rather than to lights placed off the window.
fn bar_height_or_default(persisted: Option<&str>) -> f64 {
    persisted
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|px| is_plausible_bar_height(*px))
        .unwrap_or(DEFAULT_BAR_HEIGHT_PX)
}

/// How a bar height is written to disk, and therefore also the form
/// [`crate::config_scalar::write_if_changed`] compares against what it reads
/// back. Trimmed by construction, which is that function's one requirement of a
/// caller.
fn format_bar_height(px: f64) -> String {
    format!("{px}")
}

/// Write `px` for the next launch. Best-effort: a failure is logged and
/// dropped. The cost is only that a cold launch places for the default bar
/// until the frontend reports a moment later.
fn persist_bar_height(app: &tauri::AppHandle, px: f64) {
    crate::config_scalar::write_if_changed(
        app,
        BAR_HEIGHT_FILE,
        &format_bar_height(px),
        "header-bar height",
    );
}

/// Where the cluster actually sits, read back off AppKit.
///
/// The inverse of [`inset_lights`], and the only way to tell a placement that
/// held from one AppKit put back the way it likes it. Its one caller is the
/// window-lifecycle probe, which asserts on it.
#[cfg(all(target_os = "macos", feature = "window-probe"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClusterGeometry {
    /// The cluster's vertical centre, in points below the window's top edge. A
    /// correct placement puts it on half the bar height.
    pub(crate) centre_from_top: f64,
    /// The left edge of the close button's frame, which [`LIGHTS_X_PX`] sets.
    pub(crate) left_x: f64,
}

/// Read the cluster's live geometry off `ns_window`, or `None` for a window with
/// no standard buttons.
///
/// Derived from the window's own frame, not from the container's height. It
/// stays true whether or not the container is still pinned to the top edge.
/// AppKit's own layout reads back a centre of 16pt, which is what a reverted
/// placement looks like.
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

/// The bar height to place `label` against: what its own page reported, else the
/// seed, else the compiled default.
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

/// Load the remembered bar height into the cache. Called once, from `setup`,
/// before the first [`place_all`], so the very first window is placed on the
/// user's bar rather than on the default.
pub(crate) fn load_persisted(app: &tauri::AppHandle) {
    let persisted = crate::config_scalar::path(app, BAR_HEIGHT_FILE)
        .as_deref()
        .and_then(crate::config_scalar::read);
    let px = bar_height_or_default(persisted.as_deref());
    SEED_BAR_HEIGHT.store(px.to_bits(), Ordering::SeqCst);
}

/// Place the lights on one window at ITS bar height.
///
/// The late re-apply path. It runs on every `Resized` and every `Moved`, as
/// the net behind [`watch`] (ADR 0074). A retitle is the other measured revert,
/// and [`retitle`] handles it.
pub(crate) fn place(window: &tauri::Window) {
    place_at(window, bar_height_for(window.label()));
}

/// Place the lights on every top-level app window. Used at the two moments a
/// window exists with nothing yet reported into it: startup, and just after a
/// New-Window child is built. Panel preview webviews (`url-preview-*`) are
/// skipped, exactly as in `crate::paint_title_bars`.
pub(crate) fn place_all(app: &tauri::AppHandle) {
    for (label, window) in tauri::Manager::windows(app) {
        if crate::app_window::is_app_window(&label) {
            place_at(&window, bar_height_for(&label));
        }
    }
}

/// Apply a bar height the frontend just measured: remember it, place the calling
/// window's lights, and persist it for the next cold launch.
///
/// Only the CALLING window is placed, and the height is stored under ITS label.
/// Two windows can show surfaces with different bars, so a push is never news
/// about anybody else. The same value also becomes the seed for a window that
/// has not reported yet, and the durable copy for the next cold launch.
pub(crate) fn set_bar_height(
    app: &tauri::AppHandle,
    window: &tauri::Window,
    bar_height_px: f64,
) -> Result<(), String> {
    // Off macOS there are no native window buttons to place. Inert rather than
    // an error: the frontend gates its push on `data-titlebar-overlay`, which
    // only this build stamps. Rejecting would turn a build difference into a
    // visible IPC failure if anything ever did reach here.
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
    // Deliberately AFTER the validation, so the file can only ever hold a value
    // the startup path accepts. Same ordering as `persist_title_bar_color`.
    persist_bar_height(app, bar_height_px);
    Ok(())
}

/// Off macOS there is no cluster to move, so every call site above stays
/// platform-agnostic.
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

/// Title `window` without losing the lights. Off macOS there are none to keep.
#[cfg(not(target_os = "macos"))]
pub(crate) fn retitle(window: &tauri::Window, title: String) -> Result<(), String> {
    window.set_title(&title).map_err(|e| format!("{e}"))
}

/// Title `window`, and put the lights back where the retitle moved them from.
///
/// A NEW title makes AppKit lay the titlebar out afresh, which reverts both
/// numbers to its own (the probe's `retitle` stop measures it). tao's
/// `set_title` only queues `setTitle:` for later, so a placement after it would
/// run before the revert. Both writes therefore happen here, in one step.
#[cfg(target_os = "macos")]
pub(crate) fn retitle(window: &tauri::Window, title: String) -> Result<(), String> {
    let label = window.label().to_string();
    on_ns_window(window, move |ns_window| {
        retitle_and_place(ns_window, &title, bar_height_for(&label));
    })
}

/// The AppKit half of [`retitle`]: set the title, then place. Its own function
/// so the probe runs the exact writes the client does.
#[cfg(target_os = "macos")]
pub(crate) fn retitle_and_place(ns_window: &objc2_app_kit::NSWindow, title: &str, bar_px: f64) {
    ns_window.setTitle(&objc2_foundation::NSString::from_str(title));
    inset_lights(ns_window, LIGHTS_X_PX, bar_px);
}

/// Run `apply` against `window`'s `NSWindow` on the main thread, the only thread
/// AppKit may be touched from.
///
/// Off the main thread it hops rather than giving up. `notifications.rs` bails
/// there, which suits a badge the next poll re-sends. Here a dropped write would
/// leave the lights misplaced until something resized the window.
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

/// The opaque token `addObserverForName:object:queue:usingBlock:` hands back,
/// which is the only handle that can remove that registration again.
#[cfg(target_os = "macos")]
pub(crate) type Observer =
    objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>;

/// What keeps one window's cluster placed. Both observers re-apply, at two
/// different moments.
#[cfg(target_os = "macos")]
struct Watch {
    /// Inside a live resize, before AppKit displays it. See [`observe_resizes`].
    resize: Observer,
    /// After AppKit shrinks the container back. See [`observe_relayouts`].
    relayout: Observer,
    /// The container `relayout` is scoped to. Retained, so its address cannot
    /// be reused by another view while [`watch`] compares against it.
    container: objc2::rc::Retained<objc2_app_kit::NSView>,
}

#[cfg(target_os = "macos")]
thread_local! {
    /// What [`watch`] installs, keyed by Tauri window label, so a window is
    /// watched exactly once and can be unwatched when it goes away.
    ///
    /// A `thread_local!` rather than a `static` because a `Retained` is `!Send`
    /// and every path that touches this map is on the main thread already.
    /// Registration runs inside [`place_at`], which cannot reach it without
    /// first forming a `&NSWindow`. Removal runs from the `Destroyed` arm of
    /// `on_window_event`, and AppKit posts the notification on the main thread.
    /// So there is exactly one map, owned by the thread that owns AppKit.
    static WATCHES: std::cell::RefCell<std::collections::HashMap<String, Watch>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Keep one window's cluster placed through a resize and a relayout. Idempotent
/// per window: the first placement installs both observers and every later one
/// finds them there, so [`place_at`] can call this unconditionally.
///
/// A container AppKit has replaced gets a fresh watch, since the relayout
/// observer is scoped to the old one and would hear nothing.
///
/// Called with a `&NSWindow` in hand, which is itself the evidence that we are
/// on the main thread.
#[cfg(target_os = "macos")]
fn watch(label: &str, ns_window: &objc2_app_kit::NSWindow) {
    use objc2_app_kit::NSWindowButton;

    // Only app windows, so the map holds exactly the labels [`unwatch`] is
    // called for. A `url-preview-*` panel webview is not one, and its
    // `ns_window()` is the APP window hosting it. Watching under its label
    // would register a second pair of observers on a window that has one.
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

/// Re-apply from AppKit's own `NSWindowDidResizeNotification`, synchronously,
/// so a live resize never displays the cluster at AppKit's position.
///
/// ADR 0074 records why this hooks AppKit's notification rather than Tauri's
/// `Resized` event. The notification is both late enough and early enough,
/// which had to be measured. By the time it fires AppKit has already reverted
/// BOTH numbers. No later layout pass reverts them again, so what we write gets
/// committed.
///
/// `on_window_event`'s `Resized` arm stays, because it covers one moment this
/// does not: tao emits a second, synthetic resize from
/// `windowDidExitFullscreen:`. That one is late by construction, and late is
/// right for it.
#[cfg(target_os = "macos")]
pub(crate) fn observe_resizes(
    ns_window: &objc2_app_kit::NSWindow,
    place: impl Fn(&objc2_app_kit::NSWindow) + 'static,
) -> Observer {
    let object: &objc2::runtime::AnyObject = ns_window;
    // SAFETY: AppKit's own notification name constant, never written.
    let name = unsafe { objc2_app_kit::NSWindowDidResizeNotification };
    // The block runs synchronously on the posting thread, which is the whole
    // point: a queued Tauri event lands a run-loop turn too late.
    observe(name, object, move |object| {
        // SAFETY: the observer is scoped to this one window through
        // `object:`, and it is alive because it is the one posting.
        let ns_window: &objc2_app_kit::NSWindow =
            unsafe { &*(object as *const objc2::runtime::AnyObject).cast() };
        place(ns_window);
    })
}

/// Re-apply after AppKit shrinks the titlebar container back to its own height.
///
/// An appearance change reverts the placement this way, and nothing else would
/// re-apply. The window-lifecycle probe measured it. macOS makes that change by
/// itself, for instance when "Auto" appearance follows the time of day.
///
/// It does NOT cover the resize or the retitle. The probe measured both
/// reverting the buttons' own frames after this notification's block has run.
/// [`observe_resizes`] and [`retitle`] keep owning those two.
///
/// Deferred to the main operation queue, never run inside the notification. It
/// arrives inside AppKit's own `setFrame:` on the container, and a write nested
/// there set off AppKit recursion in full screen that overflowed the stack. A
/// notification centre queue does not defer: it runs the block inline when the
/// poster is already on that queue. `addOperationWithBlock:` always enqueues.
///
/// Several notifications in one layout pass queue one placement between them.
/// Our own write posts once more, and [`inset_lights`] then writes nothing.
#[cfg(target_os = "macos")]
pub(crate) fn observe_relayouts(
    ns_window: &objc2_app_kit::NSWindow,
    container: &objc2_app_kit::NSView,
    place: impl Fn(&objc2_app_kit::NSWindow) + 'static,
) -> Observer {
    // The window we were registered for, never `container.window()`. In full
    // screen AppKit hosts the titlebar in a separate toolbar window, and a
    // placement against that one would compute from the wrong frame.
    let owner = objc2::rc::Weak::from(ns_window);
    let queued = std::rc::Rc::new(std::cell::Cell::new(false));
    let deferred = block2::RcBlock::new({
        let queued = queued.clone();
        move || {
            queued.set(false);
            // A queued placement can outlive its window.
            let Some(ns_window) = owner.load() else {
                return;
            };
            // Full screen lays the titlebar out continuously. The `Resized`
            // arm re-places once the window is back.
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

/// Register `handle` for `name` from `object` alone. The block runs
/// synchronously on the posting thread, which must be the main one, and hands
/// `handle` the posting object.
#[cfg(target_os = "macos")]
fn observe(
    name: &objc2_foundation::NSNotificationName,
    object: &objc2::runtime::AnyObject,
    handle: impl Fn(&objc2::runtime::AnyObject) + 'static,
) -> Observer {
    use objc2_foundation::{NSNotification, NSNotificationCenter};

    let block = block2::RcBlock::new(move |notification: std::ptr::NonNull<NSNotification>| {
        // AppKit posts both notifications on the main thread, and a nil queue
        // runs the block on the posting thread. Checked rather than assumed: reaching
        // into AppKit off the main thread would be unsound, and skipping costs
        // only the one placement.
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

/// Stop the notification centre delivering to `observer`.
#[cfg(target_os = "macos")]
pub(crate) fn stop_observing(observer: &Observer) {
    let observer: &objc2::runtime::AnyObject = observer.as_ref();
    // SAFETY: the token is what `addObserverForName:object:queue:usingBlock:`
    // handed back, on the same centre.
    unsafe { objc2_foundation::NSNotificationCenter::defaultCenter().removeObserver(observer) };
}

/// Drop `label`'s watch, if it has one. Main thread only, like [`WATCHES`].
#[cfg(target_os = "macos")]
fn remove_watch(label: &str) {
    if let Some(watch) = WATCHES.with_borrow_mut(|watches| watches.remove(label)) {
        stop_observing(&watch.resize);
        stop_observing(&watch.relayout);
    }
}

/// Off macOS no window was ever watched.
#[cfg(not(target_os = "macos"))]
pub(crate) fn unwatch(label: &str) {
    BAR_HEIGHTS.lock().unwrap().remove(label);
}

/// Drop a closed window's observers. Called from the `Destroyed` arm of
/// `on_window_event`, and the only thing that stops the notification centre
/// holding a registration keyed on a dead window's address. Another `NSWindow`
/// can reuse that address, and the block would then place lights on somebody
/// else's window.
#[cfg(target_os = "macos")]
pub(crate) fn unwatch(label: &str) {
    // Before the main-thread guard: a plain mutex needs no thread, and a
    // reported height left behind would be handed to the next window to take
    // this label.
    BAR_HEIGHTS.lock().unwrap().remove(label);
    // The watch map is the main thread's (see [`WATCHES`]) and
    // `on_window_event` runs there. This checks the invariant rather than
    // taking a branch we expect.
    if objc2::MainThreadMarker::new().is_none() {
        eprintln!(
            "[Tauri] Traffic-light observers for {label} left registered: not on the main thread"
        );
        return;
    }
    remove_watch(label);
}

/// close -> NSTitlebarView -> NSTitlebarContainerView. The container carries
/// the cluster's vertical position: it is pinned to the window's top edge, so
/// growing it is how the buttons move DOWN.
#[cfg(target_os = "macos")]
pub(crate) fn titlebar_container(
    close: &objc2_app_kit::NSButton,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    // SAFETY: `superview` is unsafe in the generated bindings because it is
    // unbounded in what it can return. We only read and write frames on the
    // result, and holding a button is evidence we are on the main thread.
    unsafe { close.superview().and_then(|view| view.superview()) }
}

/// Move the three window buttons to `x` and centre them on a bar
/// `bar_height_px` tall. The same shape as wry's and tao's
/// `inset_traffic_lights`: grow `NSTitlebarContainerView` and let AppKit
/// re-centre the buttons inside it.
///
/// Idempotent, which is what makes the re-apply safe to run on every event. A
/// previous run leaves the buttons' `origin.y` and their pitch unchanged, so a
/// second call reads the same inputs. It then writes nothing, which is what
/// stops [`observe_relayouts`] hearing its own write forever.
#[cfg(target_os = "macos")]
pub(crate) fn inset_lights(ns_window: &objc2_app_kit::NSWindow, x: f64, bar_height_px: f64) {
    use objc2_app_kit::NSWindowButton;

    let (Some(close), Some(miniaturize)) = (
        ns_window.standardWindowButton(NSWindowButton::CloseButton),
        ns_window.standardWindowButton(NSWindowButton::MiniaturizeButton),
    ) else {
        // An undecorated window has no standard buttons and nothing to place.
        return;
    };
    // The zoom button is absent on a non-resizable window. The other two carry
    // the pitch between them, so its absence costs nothing.
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

    // AppKit's own spacing, read rather than assumed, so the cluster keeps the
    // system's rhythm and only its origin is ours.
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

    /// The probe's measured case, end to end. AppKit lays the close button out
    /// at y 9 in a 32pt container with a 14pt frame. A 48px bar wants its centre
    /// at 24. Both come out of a 40pt container, which is what the probe read
    /// back after applying it.
    #[test]
    fn a_48px_bar_centres_the_cluster_24px_down() {
        assert_eq!(container_height(48.0, 9.0, 14.0), 40.0);
    }

    /// The property the arithmetic exists for, checked by inverting it. The
    /// cluster's centre must land on the bar's centre at every supported UI
    /// scale: the 75% minimum is a 36px bar, and the 200% maximum is 96px.
    #[test]
    fn the_cluster_centres_on_the_bar_at_every_supported_scale() {
        for bar in [36.0, 48.0, 54.0, 60.0, 66.0, 72.0, 84.0, 96.0_f64] {
            let height = container_height(bar, 9.0, 14.0);
            // The AppKit relationship this is solved from: a button's centre sits
            // `container - origin_y - height / 2` below the window's top edge.
            let centre_from_top = height - 9.0 - 7.0;
            assert_eq!(centre_from_top, bar / 2.0, "bar {bar}");
        }
    }

    /// The two AppKit terms are inputs, not constants: a titlebar laid out
    /// differently must still centre. Same bar, a taller button placed lower,
    /// and the centre still lands at 24.
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
        // Truncated by a write that died half way.
        assert_eq!(bar_height_or_default(Some("4")), DEFAULT_BAR_HEIGHT_PX);
    }

    /// A parseable number that could only be a bug degrades the same way an
    /// unparseable one does. It must not place the lights somewhere the user
    /// cannot reach them.
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
            // And through the trim `read_bar_height` applies, since a file can
            // pick up a trailing newline from an editor.
            let padded = format!(" {written}\n");
            assert_eq!(bar_height_or_default(Some(padded.trim())), px, "{px}");
        }
    }

    /// The written form has to be what the reader compares against, or
    /// `persist_bar_height`'s skip never matches and every push rewrites the
    /// file. Same trap the title-bar colour's trim closed.
    #[test]
    fn the_written_form_is_already_trimmed() {
        let written = format_bar_height(48.0);
        assert_eq!(written.trim(), written);
        assert_eq!(written, "48");
    }

    #[test]
    fn a_persisted_bar_height_round_trips_through_a_real_file() {
        let dir = std::env::temp_dir().join(format!(
            "lucidos-traffic-lights-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(BAR_HEIGHT_FILE);

        // Absent is normal, and says nothing.
        assert_eq!(crate::config_scalar::read(&path), None);
        assert_eq!(bar_height_or_default(None), DEFAULT_BAR_HEIGHT_PX);

        std::fs::write(&path, format_bar_height(72.0)).unwrap();
        let read = crate::config_scalar::read(&path);
        assert_eq!(read.as_deref(), Some("72"));
        assert_eq!(bar_height_or_default(read.as_deref()), 72.0);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Held by every test that writes the two placement globals. Cargo runs
    /// tests in threads of one process, so without it a clear or a seed write
    /// lands inside another test's assertions.
    static GLOBALS: Mutex<()> = Mutex::new(());

    /// Leaves both placement globals empty, whichever way the test ended.
    fn clear_globals() {
        BAR_HEIGHTS.lock().unwrap().clear();
        SEED_BAR_HEIGHT.store(0_f64.to_bits(), Ordering::SeqCst);
    }

    /// Two windows can show surfaces with different bars: the workspace shell
    /// renders at the user's UI scale, the picker at the browser default. One
    /// shared value let each move the other's lights.
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
        // A window nobody has reported for takes the seed, and the compiled
        // default while there is no seed either.
        assert_eq!(bar_height_for("window-2"), DEFAULT_BAR_HEIGHT_PX);
        SEED_BAR_HEIGHT.store(72_f64.to_bits(), Ordering::SeqCst);
        assert_eq!(bar_height_for("window-2"), 72.0);
        assert_eq!(bar_height_for("main"), 48.0, "a report beats the seed");
        clear_globals();
    }

    /// A label is a per-process counter, so the next `window-1` must not inherit
    /// the dead one's bar.
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
        // 75% and 200% of a 3rem bar on a 16px root.
        assert!(is_plausible_bar_height(36.0));
        assert!(is_plausible_bar_height(96.0));
        assert!(!is_plausible_bar_height(0.0));
        assert!(!is_plausible_bar_height(-1.0));
        assert!(!is_plausible_bar_height(f64::NAN));
        assert!(!is_plausible_bar_height(f64::INFINITY));
    }

    /// x is a number we chose, and the CSS reserve is arithmetic on it. If it
    /// ever moves, the fallback literal in `styles/panels/shell.css` has to
    /// move with it. The CSS suite checks that from the other side by reading
    /// this file, along with the other half of the pair: the reserve's slack is
    /// split evenly, so `--titlebar-lights-gap` has to equal this.
    #[test]
    fn the_chosen_x_is_half_the_reserves_slack() {
        // Written as the property rather than as `== 10.0`, which it implies.
        // Our x, plus the measured 60pt cluster, plus a gap the CSS holds equal
        // to the x, is the 80px the header row keeps clear. So a change to the
        // x cannot be absorbed by updating a number here. It has to say what
        // happened to the reserve.
        assert_eq!(LIGHTS_X_PX + 60.0 + LIGHTS_X_PX, 80.0);
    }
}
