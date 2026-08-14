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
//! reverts the placement on **every window resize**, so [`watch_resizes`] owns
//! re-applying it.

use std::sync::atomic::{AtomicU64, Ordering};

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

/// The bar height the lights are currently placed against, as `f64::to_bits`.
/// Seeded from disk in [`load_persisted`] and rewritten by every frontend push,
/// so the re-apply that each resize forces costs no disk read. A CACHE:
/// [`BAR_HEIGHT_FILE`] is the durable copy. `0` means "nothing loaded yet" and
/// is unambiguous, since `0.0` is not a plausible bar height.
static BAR_HEIGHT: AtomicU64 = AtomicU64::new(0);

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

/// The bar height to place with right now: the last one pushed or loaded, else
/// the compiled default.
fn current_bar_height() -> f64 {
    let px = f64::from_bits(BAR_HEIGHT.load(Ordering::SeqCst));
    if is_plausible_bar_height(px) {
        px
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
    BAR_HEIGHT.store(px.to_bits(), Ordering::SeqCst);
}

/// Place the lights on one window at the current bar height. The re-apply path:
/// called on every `Resized`, because AppKit reverts the placement on each one.
pub(crate) fn place(window: &tauri::Window) {
    place_at(window, current_bar_height());
}

/// Place the lights on every top-level app window. Used at the two moments a
/// window exists with nothing yet reported into it: startup, and just after a
/// New-Window child is built. Panel preview webviews (`url-preview-*`) are
/// skipped, exactly as in `crate::paint_title_bars`.
pub(crate) fn place_all(app: &tauri::AppHandle) {
    let bar_height_px = current_bar_height();
    for (label, window) in tauri::Manager::windows(app) {
        if crate::is_app_window(&label) {
            place_at(&window, bar_height_px);
        }
    }
}

/// Apply a bar height the frontend just measured: remember it, place the calling
/// window's lights, and persist it for the next cold launch.
///
/// Only the CALLING window is placed. Every window resolves the same bar
/// height, a function of the one device-wide UI scale, and each reports for
/// itself as it boots. Fanning out here would only re-place windows that are
/// about to say the same thing.
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
    BAR_HEIGHT.store(bar_height_px.to_bits(), Ordering::SeqCst);
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
    use objc2::MainThreadMarker;

    let Some(_mtm) = MainThreadMarker::new() else {
        // AppKit may only be touched from the main thread, and a sync
        // `#[tauri::command]` is not documented to run there, so hop instead of
        // giving up. `notifications.rs` bails in the same situation, which is
        // right for a best-effort badge that the next poll re-sends and wrong
        // here: dropping this would leave the lights where the LAST launch put
        // them until something resized the window.
        let deferred = window.clone();
        if let Err(e) = window.run_on_main_thread(move || place_at(&deferred, bar_height_px)) {
            eprintln!(
                "[Tauri] Could not marshal the traffic-light placement to the main thread: {e}"
            );
        }
        return;
    };

    let ptr = match window.ns_window() {
        Ok(ptr) if !ptr.is_null() => ptr,
        Ok(_) => return,
        Err(e) => {
            eprintln!("[Tauri] No NSWindow to place the traffic lights on: {e}");
            return;
        }
    };
    // SAFETY: `ns_window` hands back an autoreleased `NSWindow` for this window,
    // valid for the rest of this call. `_mtm` is the evidence that forming a
    // reference to a `MainThreadOnly` type here is sound.
    let ns_window: &objc2_app_kit::NSWindow = unsafe { &*ptr.cast() };
    watch_resizes(window.label(), ns_window);
    inset_lights(ns_window, LIGHTS_X_PX, bar_height_px);
}

/// The opaque token `addObserverForName:object:queue:usingBlock:` hands back,
/// which is the only handle that can remove that registration again.
#[cfg(target_os = "macos")]
type ResizeObserver =
    objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>;

#[cfg(target_os = "macos")]
thread_local! {
    /// The observers [`watch_resizes`] installs, keyed by Tauri window label, so
    /// a window is watched exactly once and can be unwatched when it goes away.
    ///
    /// A `thread_local!` rather than a `static` because a `Retained` is `!Send`
    /// and every path that touches this map is on the main thread already.
    /// Registration runs inside [`place_at`], which cannot reach it without
    /// first forming a `&NSWindow`. Removal runs from the `Destroyed` arm of
    /// `on_window_event`, and AppKit posts the notification on the main thread.
    /// So there is exactly one map, owned by the thread that owns AppKit.
    static RESIZE_OBSERVERS: std::cell::RefCell<std::collections::HashMap<String, ResizeObserver>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Keep one window's cluster placed through a live resize, by re-applying from
/// AppKit's own `NSWindowDidResizeNotification`. Idempotent per window: the
/// first placement installs the observer and every later one finds it there, so
/// [`place_at`] can call this unconditionally.
///
/// ADR 0074 records why this hooks AppKit's notification rather than Tauri's
/// `Resized` event, and which two tidier-looking hooks were probed and failed.
///
/// The notification is both late enough and early enough, which had to be
/// measured rather than reasoned about. By the time it fires AppKit has already
/// reverted BOTH numbers, so there is something to correct. No later layout
/// pass reverts them again, so what we write gets committed.
///
/// `on_window_event`'s `Resized` arm stays, because it covers one moment this
/// does not: tao emits a second, synthetic resize from
/// `windowDidExitFullscreen:`. That one is late by construction, and late is
/// right for it.
///
/// Called with a `&NSWindow` in hand, which is itself the evidence that we are
/// on the main thread.
#[cfg(target_os = "macos")]
fn watch_resizes(label: &str, ns_window: &objc2_app_kit::NSWindow) {
    use objc2_foundation::{NSNotification, NSNotificationCenter};

    // Only app windows, so the map holds exactly the labels [`unwatch`] is
    // called for. A `url-preview-*` panel webview is not one, and its
    // `ns_window()` is the APP window hosting it. Watching under its label
    // would register a second observer on a window that already has one.
    if !crate::is_app_window(label) {
        return;
    }
    if RESIZE_OBSERVERS.with_borrow(|observers| observers.contains_key(label)) {
        return;
    }

    let block = block2::RcBlock::new(|notification: std::ptr::NonNull<NSNotification>| {
        // AppKit posts this on the main thread and we registered with a nil
        // queue, so the block runs there. Checked rather than assumed: reaching
        // into AppKit off the main thread would be unsound, and skipping costs
        // only the one placement.
        if objc2::MainThreadMarker::new().is_none() {
            return;
        }
        // SAFETY: the notification is alive for the duration of the call.
        let Some(object) = (unsafe { notification.as_ref() }).object() else {
            return;
        };
        // SAFETY: the observer below is scoped to a single window through the
        // `object:` argument. The only sender that can reach this block is that
        // `NSWindow`, and it is alive because it is the one posting.
        let ns_window: &objc2_app_kit::NSWindow =
            unsafe { &*objc2::rc::Retained::as_ptr(&object).cast() };
        inset_lights(ns_window, LIGHTS_X_PX, current_bar_height());
    });

    let object: &objc2::runtime::AnyObject = ns_window;
    // SAFETY: the name is AppKit's own notification constant, and the object is
    // the window we want scoped notifications for. A nil queue asks for the
    // block to run synchronously on the posting thread, which is the whole
    // point: a queued block would land where the Tauri event already is.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSWindowDidResizeNotification),
            Some(object),
            None,
            &block,
        )
    };
    RESIZE_OBSERVERS.with_borrow_mut(|observers| observers.insert(label.to_string(), observer));
}

/// Off macOS no window was ever watched.
#[cfg(not(target_os = "macos"))]
pub(crate) fn unwatch(_label: &str) {}

/// Drop a closed window's resize observer. Called from the `Destroyed` arm of
/// `on_window_event`, and the only thing that stops the notification centre
/// holding a registration keyed on a dead window's address. Another `NSWindow`
/// can reuse that address, and the block would then place lights on somebody
/// else's window.
#[cfg(target_os = "macos")]
pub(crate) fn unwatch(label: &str) {
    // The map is the main thread's (see [`RESIZE_OBSERVERS`]) and
    // `on_window_event` runs there. This checks the invariant rather than
    // taking a branch we expect.
    if objc2::MainThreadMarker::new().is_none() {
        eprintln!("[Tauri] Traffic-light resize observer for {label} left registered: not on the main thread");
        return;
    }
    let Some(observer) = RESIZE_OBSERVERS.with_borrow_mut(|observers| observers.remove(label))
    else {
        return;
    };
    let observer: &objc2::runtime::AnyObject = observer.as_ref();
    // SAFETY: the token is what `addObserverForName:object:queue:usingBlock:`
    // handed back for this window, on the same centre.
    unsafe { objc2_foundation::NSNotificationCenter::defaultCenter().removeObserver(observer) };
}

/// Move the three window buttons to `x` and centre them on a bar
/// `bar_height_px` tall. The same shape as wry's and tao's
/// `inset_traffic_lights`: grow `NSTitlebarContainerView` and let AppKit
/// re-centre the buttons inside it.
///
/// Idempotent, which is what makes the re-apply safe to run on every event. A
/// previous run leaves the buttons' `origin.y` and their pitch unchanged, so a
/// second call reads the same inputs and writes the same frames.
#[cfg(target_os = "macos")]
fn inset_lights(ns_window: &objc2_app_kit::NSWindow, x: f64, bar_height_px: f64) {
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

    // close -> NSTitlebarView -> NSTitlebarContainerView. The container is what
    // carries the cluster's vertical position: it is pinned to the window's top
    // edge, so growing it is how the buttons move DOWN.
    // SAFETY: `superview` is unsafe in the generated bindings because it is
    // unbounded in what it can return. We only read frames off the result, and
    // we are on the main thread.
    let Some(container) = (unsafe { close.superview().and_then(|view| view.superview()) }) else {
        return;
    };

    let close_frame = close.frame();
    let height = container_height(bar_height_px, close_frame.origin.y, close_frame.size.height);
    let mut container_frame = container.frame();
    container_frame.size.height = height;
    container_frame.origin.y = ns_window.frame().size.height - height;
    container.setFrame(container_frame);

    // AppKit's own spacing, read rather than assumed, so the cluster keeps the
    // system's rhythm and only its origin is ours.
    let pitch = miniaturize.frame().origin.x - close_frame.origin.x;
    let mut buttons = vec![close, miniaturize];
    buttons.extend(zoom);
    for (index, button) in buttons.into_iter().enumerate() {
        let mut origin = button.frame().origin;
        origin.x = x + index as f64 * pitch;
        button.setFrameOrigin(origin);
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
