//! Sanity clamp on the geometry `tauri-plugin-window-state` restores.
//!
//! The plugin writes the saved rect straight onto the window, and its only guard
//! is that some currently-attached monitor `intersects` the SAVED rect. A rect
//! that is degenerate (a 1x1 window) passes that guard trivially as long as its
//! corner lands on a panel, and a rect saved against a display that is no longer
//! attached simply keeps whatever position the OS gave the window. Both leave a
//! client the user cannot find: shipped as a 1x1 window jammed in the
//! bottom-right corner of a 1728x1117 logical display, restored from a saved
//! position of 3454,2170 physical on a 3456x2234 panel.
//!
//! So between the restore and the show, the geometry is checked once against the
//! CURRENT display layout and the minimums declared in `tauri.conf.json`. A
//! healthy rect is left exactly as it is; a degenerate one falls back to the
//! declared default size; a position with no grabbable title bar left on any
//! monitor is nudged back on screen, or re-centred on the primary when the
//! window is essentially gone (the display it was on was unplugged).
//!
//! # Units
//!
//! Every number in this module is in **physical pixels**, in the desktop's
//! global coordinate space: the space `Window::outer_position` and
//! `Monitor::work_area` both speak, and the space the plugin persists and
//! restores in. It is also the only space shared by monitors running different
//! scale factors, so a mixed-DPI desktop needs no per-monitor conversion here.
//! The conversion happens once, at the boundary in [`clamp_restored_geometry`],
//! where the LOGICAL values from `tauri.conf.json` are multiplied by the
//! window's scale factor.

use tauri::{Manager, PhysicalPosition, PhysicalSize};

/// Height of the strip at the top of the window that counts as the drag handle,
/// in LOGICAL pixels: one standard macOS title bar, the same 28 the shell stamps
/// as `--titlebar-inset`. A window whose title bar is off screen cannot be moved
/// back with the pointer, which is the state this module exists to prevent.
const GRAB_HEIGHT_LOGICAL: f64 = 28.0;

/// How much of that strip has to be inside one monitor's work area for the
/// window to count as reachable, in LOGICAL pixels. The traffic-light cluster
/// plus its clearance already claims 80px of the band (the reserve
/// `store/paneMinimums.ts` restates as `TITLEBAR_LIGHTS_RESERVE_PX`), so 120
/// leaves 40px of strip that is actually grabbable rather than a button.
const GRAB_WIDTH_LOGICAL: f64 = 120.0;

/// A window or monitor rect in physical pixels. `i64` throughout so an
/// off-screen position far outside the desktop cannot underflow a subtraction
/// or overflow an area product.
///
/// Serializable because `window_session` persists one per workspace, and a
/// second rect type would be a second set of units to get wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    fn right(&self) -> i64 {
        self.x + self.width
    }

    fn bottom(&self) -> i64 {
        self.y + self.height
    }

    fn overlap_width(&self, other: &Rect) -> i64 {
        (self.right().min(other.right()) - self.x.max(other.x)).max(0)
    }

    fn overlap_height(&self, other: &Rect) -> i64 {
        (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0)
    }

    fn overlap_area(&self, other: &Rect) -> i64 {
        self.overlap_width(other) * self.overlap_height(other)
    }
}

/// The display layout the clamp judges a rect against, in physical pixels.
#[derive(Debug, Clone)]
pub(crate) struct Displays {
    /// Work area (the usable frame, menu bar and Dock excluded) of every
    /// currently-attached monitor.
    pub work_areas: Vec<Rect>,
    /// Work area of the primary monitor: where a window with nowhere left to be
    /// gets re-centred.
    pub primary: Rect,
}

/// The floors and fallbacks the clamp applies, in physical pixels. Derived from
/// `tauri.conf.json` rather than restated, so the declared minimum and the
/// minimum the clamp enforces cannot drift apart.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Policy {
    pub min_width: i64,
    pub min_height: i64,
    pub default_width: i64,
    pub default_height: i64,
    pub grab_width: i64,
    pub grab_height: i64,
}

impl Policy {
    /// Convert the LOGICAL config values plus this module's logical thresholds
    /// into the physical pixels everything else here works in.
    fn from_logical(
        min_width: f64,
        min_height: f64,
        default_width: f64,
        default_height: f64,
        scale: f64,
    ) -> Self {
        // A non-finite or non-positive scale factor would turn every threshold
        // into nonsense, so fall back to 1.0 and let the clamp reason in what
        // are then logical pixels: coarser, never wrong in a harmful direction.
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let px = |logical: f64| (logical * scale).round().max(0.0) as i64;
        Self {
            min_width: px(min_width),
            min_height: px(min_height),
            default_width: px(default_width),
            default_height: px(default_height),
            grab_width: px(GRAB_WIDTH_LOGICAL),
            grab_height: px(GRAB_HEIGHT_LOGICAL),
        }
    }
}

/// Whether the window's drag handle (the top strip of its own width) has enough
/// of itself inside one monitor's work area to be grabbed with the pointer.
///
/// The whole strip has to be inside VERTICALLY: a title bar half of which is
/// above the work area is under the menu bar, where it cannot be hit. It only
/// has to be [`Policy::grab_width`] wide HORIZONTALLY, because a window hanging
/// off the right edge of a screen is a perfectly ordinary thing for a user to
/// have arranged.
fn handle_is_reachable(rect: &Rect, displays: &Displays, policy: &Policy) -> bool {
    let handle = Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: policy.grab_height.min(rect.height),
    };
    // A window narrower than the requirement can never satisfy it, so what it
    // owes is its own full width.
    let needed_width = policy.grab_width.min(handle.width);
    displays.work_areas.iter().any(|work_area| {
        handle.overlap_width(work_area) >= needed_width
            && handle.overlap_height(work_area) >= handle.height
    })
}

/// Slide one axis of the rect so a `len`-long span sits inside the
/// `origin..origin + span` work area, or flush with its leading edge when the
/// rect is longer than the work area (a window taller than the screen keeps its
/// title bar reachable, which is the half that matters).
fn nudge_axis(pos: i64, len: i64, origin: i64, span: i64) -> i64 {
    pos.clamp(origin, origin + (span - len).max(0))
}

/// The one decision this module makes, kept pure so it is testable without an
/// NSWindow or a second display.
///
/// `restored` is the geometry the window-state plugin just put on the window.
/// Returns `Some(fixed)` when it had to be corrected and `None` when it was
/// already healthy, so a normal launch performs no window calls at all.
///
/// The result is always itself healthy, which is what makes this safe to run
/// once and never again: re-running it on its own output returns `None`.
pub(crate) fn sanitize(restored: Rect, displays: &Displays, policy: &Policy) -> Option<Rect> {
    let mut fixed = restored;

    // 1. Size. Under the declared minimum is not a shape the user can drag a
    //    window into, so it is corruption rather than a preference: fall back to
    //    the declared DEFAULT, not to the minimum, which would hand back a
    //    usable but tiny window nobody asked for.
    if fixed.width < policy.min_width || fixed.height < policy.min_height {
        fixed.width = policy.default_width;
        fixed.height = policy.default_height;
    }

    // 2. Position, judged against the size decided above: a corner position that
    //    was fine for a 1x1 window puts a full-size one almost entirely off the
    //    screen, and that is exactly the shipped bug.
    if !handle_is_reachable(&fixed, displays, policy) {
        let best = displays
            .work_areas
            .iter()
            .copied()
            .max_by_key(|work_area| fixed.overlap_area(work_area))
            // Is there still a monitor this window belongs to? It counts as
            // belonging while at least a drag handle's worth of it is on screen.
            // Deriving the threshold from the handle rather than picking a
            // percentage keeps one notion of "enough window to work with", and
            // it is what separates a window hanging off an edge (a place the
            // user put it) from the two cases that have no place left: a corner
            // sliver, and a display that was unplugged.
            .filter(|work_area| {
                fixed.overlap_area(work_area) >= policy.grab_width * policy.grab_height
            });
        match best {
            // Partly off screen: keep the user's neighbourhood, nudge it back.
            Some(work_area) => {
                fixed.x = nudge_axis(fixed.x, fixed.width, work_area.x, work_area.width);
                fixed.y = nudge_axis(fixed.y, fixed.height, work_area.y, work_area.height);
            }
            // Gone, or as good as: start over, centred on the primary monitor.
            None => {
                let primary = displays.primary;
                fixed.x = primary.x + (primary.width - fixed.width).max(0) / 2;
                fixed.y = primary.y + (primary.height - fixed.height).max(0) / 2;
            }
        }
    }

    (fixed != restored).then_some(fixed)
}

/// The declared window config to judge `label` against, falling back to the
/// declared `main` window.
///
/// Only `main` is in `tauri.conf.json`. Every other app window is built at run
/// time and labelled `window-<n>`, so a plain `find` returns `None` for one and
/// the clamp skips it. That made the clamp a no-op for the windows a session
/// restore places, which are the ones whose rect came off a file on disk.
///
/// Falling back is right rather than merely convenient: `open_app_window`
/// builds every extra window from the same defaults, so `main`'s declared
/// minimums and default size are the policy for all of them.
fn policy_config<'a>(
    windows: &'a [tauri::utils::config::WindowConfig],
    label: &str,
) -> Option<&'a tauri::utils::config::WindowConfig> {
    windows.iter().find(|w| w.label == label).or_else(|| {
        windows
            .iter()
            .find(|w| w.label == crate::app_window::MAIN_WINDOW_LABEL)
    })
}

/// The LOGICAL minimum size `tauri.conf.json` declares, or `None` when it
/// declares neither half.
///
/// `open_app_window` applies it to every window it builds, which is what makes
/// the clamp's minimum test sound for one. That test reads a frame under the
/// minimum as corruption. A window the user could legitimately drag that small
/// would then be snapped to the default size on its next restore.
pub(crate) fn declared_min_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    let config = policy_config(
        &app.config().app.windows,
        crate::app_window::MAIN_WINDOW_LABEL,
    )?;
    Some((config.min_width?, config.min_height?))
}

/// Read a window's restored geometry, sanity-check it, and correct it in place
/// if it is unusable. Called for `main` from `setup`, which is after the
/// window-state plugin's `on_window_ready` restore and before anything shows the
/// window (it is declared `visible: false` and shown by `show_startup_window`).
/// Also called for each window a session restore places, while it is hidden.
///
/// Every failure here is a no-op with a log line: a client that cannot read its
/// own monitors must still come up.
///
/// By window, not webview window, per ADR 0140. Every read and every correction
/// here is a window operation. Both callers run before their window is on
/// screen, so no preview can be attached yet.
pub(crate) fn clamp_restored_geometry(app: &tauri::AppHandle, label: &str) {
    let Some(window) = app.get_window(label) else {
        return;
    };
    // `window_state_flags` restores FULLSCREEN as well, and a fullscreen window
    // is the one case where being outside every work area is CORRECT: macOS
    // gives it the whole screen, menu-bar strip included, so its title strip
    // fails the reachability check by construction (pinned by
    // `a_fullscreen_frame_reads_as_unreachable_which_is_why_it_is_skipped`).
    // Clamping it would move a window the plugin deliberately put there, and the
    // AppKit transition is asynchronous, so the correction would land mid-flight.
    // tao records the flag synchronously inside `set_fullscreen`, so this reads
    // true from the moment the plugin asked for it rather than when the
    // animation ends.
    //
    // MAXIMIZED is deliberately NOT skipped: a genuinely maximized window's rect
    // IS the work area, which `sanitize` passes through untouched, while a
    // window that merely CLAIMS to be maximized while sitting in a corner is
    // exactly the state this exists to fix.
    if window.is_fullscreen().unwrap_or(false) {
        return;
    }
    let Some(config) = policy_config(&app.config().app.windows, label).cloned() else {
        eprintln!("[Tauri] No window config to clamp `{label}` against: skipping the clamp");
        return;
    };

    // Scale factor of the monitor the window currently sits on. A window on the
    // wrong monitor may report the wrong one, which only scales the thresholds;
    // this is a coarse sanity check, not a layout.
    let scale = window.scale_factor().unwrap_or(1.0);
    // Absent minimums mean no floor rather than a guessed one, so removing them
    // from the config can never shrink a window. `tauri_conf_declares_minimums`
    // below is what keeps them declared.
    let policy = Policy::from_logical(
        config.min_width.unwrap_or(0.0),
        config.min_height.unwrap_or(0.0),
        config.width,
        config.height,
        scale,
    );

    let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else {
        eprintln!("[Tauri] Could not read the restored window geometry: skipping the clamp");
        return;
    };
    // `outer_position` + `inner_size` is deliberately the pair the window-state
    // plugin itself persists and restores, so the clamp reasons about the exact
    // numbers that produced the bad state. On this window the two sizes are the
    // same anyway: `titleBarStyle: "Overlay"` gives the content view the full
    // frame, so there is no title bar outside it.
    let restored = Rect {
        x: position.x as i64,
        y: position.y as i64,
        width: size.width as i64,
        height: size.height as i64,
    };

    let Ok(monitors) = window.available_monitors() else {
        eprintln!("[Tauri] Could not enumerate monitors: skipping the restore clamp");
        return;
    };
    let primary = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| monitors.first().cloned());
    let Some(primary) = primary else {
        eprintln!("[Tauri] No monitor to place the window on: skipping the restore clamp");
        return;
    };
    let displays = Displays {
        work_areas: monitors.iter().map(work_area_rect).collect(),
        primary: work_area_rect(&primary),
    };

    let Some(fixed) = sanitize(restored, &displays, &policy) else {
        return;
    };
    eprintln!(
        "[Tauri] Restored window geometry {}x{} at {},{} is unusable on the attached displays: \
         correcting to {}x{} at {},{} (physical pixels)",
        restored.width,
        restored.height,
        restored.x,
        restored.y,
        fixed.width,
        fixed.height,
        fixed.x,
        fixed.y
    );
    if let Err(e) = window.set_size(PhysicalSize::new(fixed.width as u32, fixed.height as u32)) {
        eprintln!("[Tauri] Failed to correct the restored window size: {e}");
    }
    if let Err(e) = window.set_position(PhysicalPosition::new(fixed.x as i32, fixed.y as i32)) {
        eprintln!("[Tauri] Failed to correct the restored window position: {e}");
    }
}

/// A monitor's usable frame as a [`Rect`]. Deliberately the work area rather
/// than the full resolution: the menu bar and the Dock are not places a title
/// bar can be grabbed.
fn work_area_rect(monitor: &tauri::Monitor) -> Rect {
    let area = monitor.work_area();
    Rect {
        x: area.position.x as i64,
        y: area.position.y as i64,
        width: area.size.width as i64,
        height: area.size.height as i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Which config the clamp judges a window against ───────────────────────

    fn window_config(label: &str) -> tauri::utils::config::WindowConfig {
        tauri::utils::config::WindowConfig {
            label: label.to_string(),
            ..Default::default()
        }
    }

    // The clamp used to `find` the exact label and skip when it missed. Only
    // `main` is declared, so it skipped every `window-<n>`: precisely the
    // windows a session restore places from a rect read off disk.
    #[test]
    fn a_runtime_window_is_judged_against_the_declared_main_config() {
        let windows = [window_config("main")];
        assert_eq!(
            policy_config(&windows, "window-0").map(|w| w.label.as_str()),
            Some("main"),
        );
    }

    #[test]
    fn a_declared_window_keeps_its_own_config() {
        let windows = [window_config("main"), window_config("other")];
        assert_eq!(
            policy_config(&windows, "other").map(|w| w.label.as_str()),
            Some("other"),
        );
    }

    // No config at all is the one case with no policy to apply, and the clamp
    // then skips rather than inventing minimums.
    #[test]
    fn no_declared_window_leaves_nothing_to_clamp_against() {
        assert!(policy_config(&[], "main").is_none());
    }

    /// A single 3456x2234 physical panel with a 74px menu bar, the display the
    /// bug was reported on.
    fn one_panel() -> Displays {
        let work_area = Rect {
            x: 0,
            y: 74,
            width: 3456,
            height: 2160,
        };
        Displays {
            work_areas: vec![work_area],
            primary: work_area,
        }
    }

    /// The panel above plus an external display to its right, for the
    /// unplug case.
    fn two_panels() -> Displays {
        let mut displays = one_panel();
        displays.work_areas.push(Rect {
            x: 3456,
            y: 0,
            width: 2560,
            height: 1440,
        });
        displays
    }

    /// `tauri.conf.json`'s 480x400 minimum and 1024x768 default at a 2x panel.
    fn policy() -> Policy {
        Policy::from_logical(480.0, 400.0, 1024.0, 768.0, 2.0)
    }

    #[test]
    fn a_healthy_rect_is_left_untouched() {
        let healthy = Rect {
            x: 400,
            y: 300,
            width: 2048,
            height: 1536,
        };
        assert_eq!(sanitize(healthy, &one_panel(), &policy()), None);
    }

    #[test]
    fn a_window_filling_the_work_area_is_left_untouched() {
        let displays = one_panel();
        assert_eq!(sanitize(displays.primary, &displays, &policy()), None);
    }

    #[test]
    fn a_degenerate_size_falls_back_to_the_declared_default() {
        // 1x1 logical in the middle of the screen: only the size is wrong.
        let degenerate = Rect {
            x: 1000,
            y: 800,
            width: 2,
            height: 2,
        };
        let fixed = sanitize(degenerate, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (2048, 1536));
        // Still reachable at that position, so the position is not touched.
        assert_eq!((fixed.x, fixed.y), (1000, 800));
    }

    #[test]
    fn the_reported_bug_recentres_on_the_primary() {
        // 1x1 logical at 1727,1085 logical: the saved 3454,2170 physical, the
        // extreme bottom-right of the panel.
        let shipped_bug = Rect {
            x: 3454,
            y: 2170,
            width: 2,
            height: 2,
        };
        let fixed = sanitize(shipped_bug, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (2048, 1536));
        assert_eq!((fixed.x, fixed.y), (704, 386));
    }

    #[test]
    fn a_window_on_an_unplugged_display_recentres_on_the_primary() {
        // Saved on the external display, which is no longer attached.
        let orphaned = Rect {
            x: 4000,
            y: 200,
            width: 2048,
            height: 1536,
        };
        assert_eq!(sanitize(orphaned, &two_panels(), &policy()), None);
        let fixed = sanitize(orphaned, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (2048, 1536));
        assert_eq!((fixed.x, fixed.y), (704, 386));
    }

    #[test]
    fn a_partly_offscreen_window_is_nudged_back_rather_than_recentred() {
        // Hanging off the right edge with only 100px of title bar left: too
        // little to grab, but most of the window is still on screen.
        let hanging = Rect {
            x: 3356,
            y: 500,
            width: 2048,
            height: 1536,
        };
        let fixed = sanitize(hanging, &one_panel(), &policy()).expect("must be corrected");
        // Pushed just far enough left to sit inside the work area, keeping the
        // size and the vertical position the user chose.
        assert_eq!((fixed.x, fixed.y), (1408, 500));
        assert_eq!((fixed.width, fixed.height), (2048, 1536));
    }

    #[test]
    fn a_sliver_on_the_edge_counts_as_gone_and_recentres() {
        // Two physical pixels of a full-size window left on the panel: less than
        // a drag handle's worth, so there is no neighbourhood left to nudge into.
        let sliver = Rect {
            x: 3454,
            y: 500,
            width: 2048,
            height: 1536,
        };
        let fixed = sanitize(sliver, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.x, fixed.y), (704, 386));
    }

    #[test]
    fn a_title_bar_above_the_work_area_is_nudged_down() {
        // Top edge under the menu bar: horizontally fine, vertically not.
        let under_the_menu_bar = Rect {
            x: 400,
            y: 0,
            width: 2048,
            height: 1536,
        };
        let fixed =
            sanitize(under_the_menu_bar, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.x, fixed.y), (400, 74));
    }

    #[test]
    fn a_window_larger_than_the_work_area_aligns_with_its_leading_edge() {
        let oversized = Rect {
            x: -900,
            y: -200,
            width: 4000,
            height: 2400,
        };
        let fixed = sanitize(oversized, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.x, fixed.y), (0, 74));
        // Oversized is not degenerate, so the size the user had is kept.
        assert_eq!((fixed.width, fixed.height), (4000, 2400));
    }

    /// The reason `clamp_restored_geometry` returns early on a fullscreen
    /// window. A macOS fullscreen frame is the whole SCREEN, menu-bar strip
    /// included, so it is outside the work area at the top and the grab band
    /// reads as unreachable. Nothing is wrong with it; the clamp simply has no
    /// business judging it.
    #[test]
    fn a_fullscreen_frame_reads_as_unreachable_which_is_why_it_is_skipped() {
        let fullscreen = Rect {
            x: 0,
            y: 0,
            width: 3456,
            height: 2234,
        };
        assert!(sanitize(fullscreen, &one_panel(), &policy()).is_some());
    }

    #[test]
    fn a_window_on_the_second_display_keeps_its_place() {
        let external = Rect {
            x: 3700,
            y: 100,
            width: 2048,
            height: 1200,
        };
        assert_eq!(sanitize(external, &two_panels(), &policy()), None);
    }

    #[test]
    fn a_correction_is_itself_healthy_so_the_clamp_cannot_loop() {
        let broken = [
            Rect {
                x: 3454,
                y: 2170,
                width: 2,
                height: 2,
            },
            Rect {
                x: 4000,
                y: 200,
                width: 2048,
                height: 1536,
            },
            Rect {
                x: 3356,
                y: 500,
                width: 2048,
                height: 1536,
            },
            Rect {
                x: -900,
                y: -200,
                width: 4000,
                height: 2400,
            },
        ];
        for rect in broken {
            let fixed = sanitize(rect, &one_panel(), &policy()).expect("must be corrected");
            assert_eq!(
                sanitize(fixed, &one_panel(), &policy()),
                None,
                "correcting {rect:?} produced {fixed:?}, which needs correcting again"
            );
        }
    }

    #[test]
    fn a_1x_display_gets_1x_thresholds() {
        let policy = Policy::from_logical(480.0, 400.0, 1024.0, 768.0, 1.0);
        assert_eq!((policy.min_width, policy.min_height), (480, 400));
        assert_eq!((policy.default_width, policy.default_height), (1024, 768));
        assert_eq!((policy.grab_width, policy.grab_height), (120, 28));
    }

    #[test]
    fn a_nonsense_scale_factor_degrades_to_1x() {
        let policy = Policy::from_logical(480.0, 400.0, 1024.0, 768.0, 0.0);
        assert_eq!((policy.min_width, policy.min_height), (480, 400));
        let policy = Policy::from_logical(480.0, 400.0, 1024.0, 768.0, f64::NAN);
        assert_eq!((policy.min_width, policy.min_height), (480, 400));
    }

    /// The clamp reads its floor from the config, so a config that declares no
    /// minimum silently disables half of the fix. It also has to stay a floor
    /// the layout can actually serve: the narrowest layout the stylesheets
    /// author is the `max-width: 600px` block in `styles/mobile.css`, which is
    /// reasoned about down to a 375px phone, so 480 sits above what the layout
    /// is exercised at and below the breakpoint that selects it.
    #[test]
    fn tauri_conf_declares_minimums_the_clamp_can_read() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let main = config["app"]["windows"]
            .as_array()
            .and_then(|windows| windows.first())
            .expect("a main window");
        let number = |key: &str| {
            main[key]
                .as_f64()
                .unwrap_or_else(|| panic!("{key} declared"))
        };
        assert_eq!(number("minWidth"), 480.0);
        assert_eq!(number("minHeight"), 400.0);
        // The fallback has to clear the floor, or a degenerate rect would be
        // corrected into another degenerate one.
        assert!(number("width") >= number("minWidth"));
        assert!(number("height") >= number("minHeight"));
    }
}
