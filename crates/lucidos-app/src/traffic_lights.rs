//! Where the macOS traffic lights sit.
//!
//! Under `titleBarStyle: "Overlay"` the webview owns the full window height and
//! the three window buttons float above it in an AppKit layer, so our HTML never
//! reflows around them. That leaves two numbers to own, and this module owns
//! both: the x the cluster starts at ([`LIGHTS_X_PX`], which
//! `styles/panels/shell.css` turns into `--titlebar-lights-reserve`) and the y
//! that puts the cluster's vertical centre on the centre of our own header bar,
//! so the lights obey the same centring rule (`--header-band-lift`) as every
//! other control in the row instead of being the one control sitting higher than
//! its neighbours.
//!
//! **We place them ourselves, through `Window::ns_window()`, because Tauri
//! 2.11.4 exposes no runtime setter.** `WebviewWindowBuilder::traffic_light_position`
//! exists but is creation-time, and the `main` window is declared in
//! `tauri.conf.json` rather than built with that builder, so it has no
//! creation-time hook at all beyond a static config literal that could carry
//! neither a persisted value nor the live UI scale. One rung down,
//! `WindowDispatch::set_traffic_light_position` is declared in `tauri-runtime`
//! and implemented in `tauri-runtime-wry`, but `tauri::Window` wraps no public
//! method for it and its dispatcher is private. The crate already drives AppKit
//! through `objc2` for the Dock badge and the tray (see `notifications.rs`), so
//! doing the placement here costs one small function and buys the thing the
//! builder cannot give: a value the frontend can re-push whenever the UI scale
//! changes.
//!
//! Deliberately we do NOT also pass `traffic_light_position` to the builder for
//! New-Window children. That would install wry's own `drawRect:` re-apply
//! holding the CREATION-time value, which would then fight every later push.
//! One mechanism, applied at every moment that needs it.
//!
//! ## The geometry, measured rather than assumed
//!
//! An AppKit probe against a window with this build's style mask (`.titled`
//! `.closable` `.miniaturizable` `.resizable` `.fullSizeContentView`,
//! transparent titlebar, hidden title) reports: a 14pt button frame carrying the
//! 12pt drawn circle, a 23pt pitch between buttons, a default frame origin of
//! (9, 9) inside `NSTitlebarView`, a 32pt `NSTitlebarContainerView`, and
//! therefore a cluster 60pt wide whose centre sits 16pt below the window's top
//! edge. None of those are the numbers the old CSS comment carried (it said 12pt
//! buttons 20pt apart starting at x 20, ending near x 66).
//!
//! Two consequences run through the rest of this file. The buttons' `origin.y`
//! INSIDE the titlebar view is not ours to set: AppKit owns it, and resizing the
//! container is what moves them, which is why [`container_height`] is the
//! arithmetic rather than a y offset. And AppKit puts everything back the way it
//! likes it on **every window resize** (measured: a plain `setFrame:` reverts
//! x to 9 and the container to 32pt), fullscreen enter/exit included, which is
//! why `on_window_event` re-applies on `Resized` and why both wry and tao run
//! their own version of this from a view's `drawRect:`.

use std::sync::atomic::{AtomicU64, Ordering};

/// Where the LEFT edge of the traffic-light cluster goes, in logical px from the
/// window's left edge. Ours to choose, and one pixel off AppKit's own, which
/// puts this window style's close button frame at x 9.
///
/// It is half of the slack in the 80px the header row keeps clear, the other
/// half being `--titlebar-lights-gap` in `styles/panels/shell.css`. 10 and 10
/// around a 60pt cluster is what centres the lights in that room: the 12pt
/// circle sits 1pt inside its 14pt frame, so the drawn cluster spans 11 to 69
/// and has the same 11px of air on each side. It used to be 20 with the gap at
/// 0, which put all the slack in front of the lights and landed the leading
/// control's box exactly where the zoom button's ended, and the user reported
/// the row reading cramped against them.
///
/// The single source for the number. [`crate::titlebar_inset_script`] stamps it
/// into `--titlebar-lights-x` before first paint and
/// `styles/panels/shell.css` derives `--titlebar-lights-reserve` from it, so the
/// room the header row keeps clear is arithmetic on the x we actually applied
/// instead of a guess at where the OS left the buttons. The cluster's own 60pt
/// width is NOT here: it is a measured AppKit fact rather than a choice, this
/// module never needs it (the pitch is read off the buttons at placement time),
/// and CSS is the only consumer.
///
/// Gated the way `notifications.rs` gates its own platform-free helpers: nothing
/// off macOS has window buttons to place, so an ungated constant is dead code on
/// any other target, and `test` keeps it available to the unit tests, which run
/// everywhere.
#[cfg(any(target_os = "macos", test))]
pub(crate) const LIGHTS_X_PX: f64 = 10.0;

/// The bar height to place against before any frontend has reported one: the
/// desktop bar at the default UI scale (`--desktop-bar-height`, `3rem`, on a
/// 16px root). The FALLBACK only, for a first run and an unreadable file. Once
/// the frontend has reported a height it is remembered ([`BAR_HEIGHT_FILE`]) and
/// that is what the next cold launch places with, so a user at 150% is not
/// launched into lights centred for 48px.
const DEFAULT_BAR_HEIGHT_PX: f64 = 48.0;

/// Bounds on a bar height we are willing to place lights against. The supported
/// UI-scale range (75% to 200%, `UI_SCALE_MIN` / `UI_SCALE_MAX` in
/// `store/actions/preferences.ts`) puts the real bar between 36px and 96px;
/// these are deliberately much wider, because the Style Remote can retune the
/// tokens the bar is built from and this is not the place to second-guess the
/// frontend's own measurement. They exist to reject a value that could only be a
/// bug: a zero, a negative, a NaN, a misplaced decimal point.
const MIN_BAR_HEIGHT_PX: f64 = 16.0;
const MAX_BAR_HEIGHT_PX: f64 = 400.0;

/// The bar height the lights are currently placed against, as `f64::to_bits`.
/// Seeded from disk in [`load_persisted`] and rewritten by every frontend push,
/// so the re-apply that each resize forces costs no disk read. A CACHE:
/// [`BAR_HEIGHT_FILE`] is the durable copy. `0` means "nothing loaded yet" and
/// is unambiguous, since `0.0` is not a plausible bar height.
static BAR_HEIGHT: AtomicU64 = AtomicU64::new(0);

/// Remembers the last bar height the frontend reported, so a cold launch can
/// place the lights on the user's bar rather than on the compiled default. A
/// bare number, no schema: one value, read back through the same plausibility
/// check anything else is, never trusted into anything but arithmetic.
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
/// The container is pinned to the window's top edge and AppKit keeps the buttons
/// vertically centred inside it, leaving their `origin.y` within the titlebar
/// view untouched at whatever it laid them out at. So a button's centre sits
/// `container_height - button_origin_y - button_height / 2` below the window's
/// top edge, and solving that for the container height is this. Both AppKit
/// terms are READ at the call site rather than baked in as the 9 and 14 the
/// probe measured, so a macOS release that retunes the titlebar keeps the
/// cluster centred instead of quietly drifting.
///
/// The 14pt frame and the 12pt circle drawn inside it share a centre, so
/// centring the frame centres the light.
///
/// Gated like [`LIGHTS_X_PX`]: its only non-test caller is the macOS placement.
#[cfg(any(target_os = "macos", test))]
fn container_height(bar_height_px: f64, button_origin_y: f64, button_height: f64) -> f64 {
    bar_height_px / 2.0 + button_origin_y + button_height / 2.0
}

/// Pure: which bar height to place with, given the raw file content from the
/// last push. The persisted value wins only if it still parses and is still
/// plausible, so a truncated or hand-edited file degrades to
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

/// Write `px` for the next launch. Best-effort: a failure is logged and dropped,
/// because the cost is only that a cold launch places the lights for the default
/// bar until the frontend reports in a moment later.
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
/// Only the CALLING window is placed. Every window resolves the same bar height
/// (it is a function of the UI scale, which is one preference for the device),
/// and each one reports for itself as it boots, so fanning out here would only
/// re-place windows that are about to say the same thing.
pub(crate) fn set_bar_height(
    app: &tauri::AppHandle,
    window: &tauri::Window,
    bar_height_px: f64,
) -> Result<(), String> {
    // Off macOS there are no native window buttons to place. Inert rather than
    // an error: the frontend gates its push on `data-titlebar-overlay`, which
    // only this build stamps, so nothing reaches here, and rejecting would turn
    // a build difference into a visible IPC failure if anything ever did.
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
    // valid for the rest of this call, and `_mtm` is the evidence that forming a
    // reference to a `MainThreadOnly` type here is sound.
    let ns_window: &objc2_app_kit::NSWindow = unsafe { &*ptr.cast() };
    inset_lights(ns_window, LIGHTS_X_PX, bar_height_px);
}

/// Move the three window buttons to `x` and centre them on a bar `bar_height_px`
/// tall. The same shape as wry's and tao's `inset_traffic_lights`, which is the
/// known-good way to do this: grow `NSTitlebarContainerView` and let AppKit
/// re-centre the buttons inside it, rather than setting their `origin.y`, which
/// AppKit owns and would fight.
///
/// Idempotent, which is what makes the `Resized` re-apply safe to run on every
/// event: the buttons' `origin.y` and the pitch between them are unchanged by a
/// previous run, so a second call reads the same inputs and writes the same
/// frames.
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
    // unbounded in what it can return; we only read frames off the result, and
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

    /// The probe's measured case, end to end: AppKit lays the close button out
    /// at y 9 in a 32pt container with a 14pt frame, and a 48px bar (3rem at the
    /// default UI scale) wants its centre at 24. Both of those come out of a
    /// 40pt container, which is exactly what the probe read back after applying
    /// it.
    #[test]
    fn a_48px_bar_centres_the_cluster_24px_down() {
        assert_eq!(container_height(48.0, 9.0, 14.0), 40.0);
    }

    /// The property the arithmetic exists for, checked by inverting it: the
    /// cluster's centre must land on the bar's centre at every UI scale the app
    /// supports, from the 75% minimum (a 36px bar) to the 200% maximum (96px).
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
    /// unparseable one does, rather than placing the lights somewhere the user
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
    /// ever moves, the fallback literal in `styles/panels/shell.css` has to move
    /// with it -- which the CSS suite checks from the other side, by reading
    /// this file, along with the other half of the pair: the reserve's slack is
    /// split evenly, so `--titlebar-lights-gap` has to equal this.
    #[test]
    fn the_chosen_x_is_half_the_reserves_slack() {
        // Written as the property rather than as `== 10.0`, which it implies:
        // our x, plus the measured 60pt cluster, plus a gap the CSS holds equal
        // to the x, is the 80px the header row has always kept clear. So a
        // change to the x cannot be absorbed by updating an expected number
        // here; it has to say what happened to the reserve.
        assert_eq!(LIGHTS_X_PX + 60.0 + LIGHTS_X_PX, 80.0);
    }
}
