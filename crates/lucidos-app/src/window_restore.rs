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
//! healthy rect is left exactly as it is. A degenerate one falls back to the
//! declared default size. A frame no attached SCREEN can hold is capped to the
//! work area it lands on. A position with no grabbable title bar left on any
//! monitor is nudged back on screen. A window that is essentially gone is
//! re-centred on the primary, which covers the display it was on being
//! unplugged.
//!
//! # Units
//!
//! Every number in this module is in **logical points**, in the macOS global
//! desktop coordinate space. macOS lays its displays out in that space and they
//! tile without overlapping, so it is the one reading a mixed-DPI desk cannot
//! break. It is also what `tauri.conf.json` already declares.
//!
//! **tao's "physical pixels" are not one space.** It derives a window's
//! position and a monitor's position from the same point space, each multiplied
//! by the scale factor of the object being read. A 1x display and a 2x display
//! therefore get two different projections, and a rect from one means nothing
//! in the other. Applying a 2x frame to a window born on a 1x display doubled
//! its size and its offset from the origin (ADR 0173).
//!
//! So every physical number converts the moment it is read, through the scale
//! factor of the thing that reported it: [`Rect::from_physical`] for a window,
//! [`panel_points`] for a monitor. Corrections go back out through
//! `app_window::place_window`, which applies logical values and moves a window
//! before it resizes it (ADR 0178).

use tauri::{Manager, PhysicalPosition, PhysicalSize};

/// Height of the strip at the top of the window that counts as the drag handle:
/// one standard macOS title bar, the same 28 the shell stamps as
/// `--titlebar-inset`. A window whose title bar is off screen cannot be moved
/// back with the pointer, which is the state this module exists to prevent.
const GRAB_HEIGHT_POINTS: f64 = 28.0;

/// How much of that strip has to be inside one monitor's work area for the
/// window to count as reachable. The traffic-light cluster plus its clearance
/// already claims 80 points of the band (`store/paneMinimums.ts` restates that
/// reserve as `TITLEBAR_LIGHTS_RESERVE_PX`). So 120 leaves 40 points of strip
/// that is grabbable rather than a button.
const GRAB_WIDTH_POINTS: f64 = 120.0;

/// A window or monitor rect in logical points. `i64` throughout so an
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
    /// The point-space rect behind a tao PHYSICAL reading, given the scale
    /// factor of whatever reported it.
    ///
    /// The one door a physical number comes through, so the module header's
    /// rule holds by construction. `scale` must belong to the OBJECT that was
    /// read: a window's own factor for a window, a monitor's own for a monitor.
    /// Another object's is what put a remembered window off screen at twice its
    /// size.
    pub(crate) fn from_physical(
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
        scale: f64,
    ) -> Self {
        // Both callers refuse to convert without a scale they could read, so
        // this is the last net rather than the first. 1.0 leaves the numbers as
        // they came: coarse on a Retina panel, never nonsense.
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let points = |physical: f64| (physical / scale).round() as i64;
        Self {
            x: points(position.x as f64),
            y: points(position.y as f64),
            width: points(size.width as f64),
            height: points(size.height as f64),
        }
    }

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

/// One attached monitor, in logical points.
///
/// Two rects, because the clamp asks a display two different questions and the
/// answers differ by the Dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Panel {
    /// The usable frame, menu bar and Dock excluded. Where a window is PLACED,
    /// since neither of those is somewhere a title bar can be grabbed.
    pub work_area: Rect,
    /// The whole screen. What bounds a RESIZE, which the Dock does not: the
    /// user can drag a window's bottom edge under it, and turning Dock
    /// auto-hide off shrinks the work area under a window already that tall.
    /// Judging a SIZE against the work area would read both as corruption.
    pub frame: Rect,
}

/// The display layout the clamp judges a rect against, in logical points.
#[derive(Debug, Clone)]
pub(crate) struct Displays {
    /// Every currently-attached monitor.
    pub panels: Vec<Panel>,
    /// Work area of the primary monitor: where a window with nowhere left to be
    /// gets re-centred.
    pub primary: Rect,
}

/// The floors and fallbacks the clamp applies, in logical points. Derived from
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
    /// Round the declared config values, which are already points, into the
    /// whole points everything here works in. No scale factor appears, which is
    /// the point of the module speaking the space the config is written in.
    fn from_declared(
        min_width: f64,
        min_height: f64,
        default_width: f64,
        default_height: f64,
    ) -> Self {
        // `max` returns the operand that is not NaN, so a garbage config value
        // becomes 0: no floor rather than a nonsense one.
        let whole = |points: f64| points.round().max(0.0) as i64;
        Self {
            min_width: whole(min_width),
            min_height: whole(min_height),
            default_width: whole(default_width),
            default_height: whole(default_height),
            grab_width: whole(GRAB_WIDTH_POINTS),
            grab_height: whole(GRAB_HEIGHT_POINTS),
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
    displays.panels.iter().any(|panel| {
        handle.overlap_width(&panel.work_area) >= needed_width
            && handle.overlap_height(&panel.work_area) >= handle.height
    })
}

/// Can any attached SCREEN hold a window of `rect`'s size?
///
/// Size only. Where the window sits is the position pass's question, and a
/// window hanging off an edge is a place the user put it.
///
/// The screen rather than the work area, because the screen is what bounds a
/// drag. See [`Panel::frame`].
fn a_screen_can_hold(rect: &Rect, displays: &Displays) -> bool {
    displays
        .panels
        .iter()
        .any(|panel| panel.frame.width >= rect.width && panel.frame.height >= rect.height)
}

/// The monitor a rect belongs to, for a correction that has to pick one.
///
/// The one it overlaps most, while that overlap is at least a drag handle's
/// worth. Deriving the threshold from the handle rather than picking a
/// percentage keeps one notion of "enough window to work with". It is also what
/// separates a window hanging off an edge from the two cases with no place left:
/// a corner sliver, and a display that was unplugged.
///
/// Answers the WORK AREA, because every caller is putting a window somewhere it
/// has to live. `None` is those two cases, and both start over on the primary.
fn home_work_area(rect: &Rect, displays: &Displays, policy: &Policy) -> Option<Rect> {
    displays
        .panels
        .iter()
        .map(|panel| panel.work_area)
        .max_by_key(|work_area| rect.overlap_area(work_area))
        .filter(|work_area| rect.overlap_area(work_area) >= policy.grab_width * policy.grab_height)
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

    // 2. Size ceiling. A window bigger than EVERY attached SCREEN is not a shape
    //    the user can have dragged, since macOS bounds a resize to the screen.
    //    A physical-pixel record produces it whenever it is restored against a
    //    scale factor the capture did not use (ADR 0173). The part of the app
    //    past the edge cannot be reached at all.
    //
    //    The screen, deliberately, not the work area: the Dock does not bound a
    //    resize. See [`Panel::frame`]. The CAP is the work area, because that is
    //    where a corrected window has to be able to live.
    //
    //    A frame that still fits SOME attached display is left alone. So a
    //    window sized for a monitor that is away today comes back whole when it
    //    returns. ADR 0193 weighs that against the stricter rule.
    //
    //    Capped AND placed together, against the one display. A shrink alone
    //    leaves the window hanging off the same edge. Its grab band is still
    //    reachable there, so the position pass below would pass it through. The
    //    floor is re-applied so step 1 cannot bounce the result back to the
    //    default on a re-run, which is what keeps this idempotent.
    if !a_screen_can_hold(&fixed, displays) {
        let home = home_work_area(&fixed, displays, policy).unwrap_or(displays.primary);
        fixed.width = fixed.width.min(home.width).max(policy.min_width);
        fixed.height = fixed.height.min(home.height).max(policy.min_height);
        fixed.x = nudge_axis(fixed.x, fixed.width, home.x, home.width);
        fixed.y = nudge_axis(fixed.y, fixed.height, home.y, home.height);
    }

    // 3. Position, judged against the size decided above: a corner position that
    //    was fine for a 1x1 window puts a full-size one almost entirely off the
    //    screen, and that is exactly the shipped bug.
    if !handle_is_reachable(&fixed, displays, policy) {
        match home_work_area(&fixed, displays, policy) {
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
/// if it is unusable.
///
/// Every caller runs just before a window reaches the screen, which is the rule
/// (ADR 0193). Four of them: `show_startup_window`, `reopen_client`,
/// `open_app_window`, and `front_window` for one brought forward that is not up
/// yet. NOT `setup`, where tao's deferred setters have not landed and the read
/// is of the geometry the window was born at.
///
/// A window already ON screen is not a caller, and `front_window` gates on that
/// for the reason its own comment gives.
///
/// Every failure here is a no-op with a log line: a client that cannot read its
/// own monitors must still come up.
///
/// By window, not webview window, per ADR 0140. Every read and every correction
/// here is a window operation.
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

    // Absent minimums mean no floor rather than a guessed one, so removing them
    // from the config can never shrink a window. `tauri_conf_declares_minimums`
    // below is what keeps them declared.
    let policy = Policy::from_declared(
        config.min_width.unwrap_or(0.0),
        config.min_height.unwrap_or(0.0),
        config.width,
        config.height,
    );

    // `outer_position` + `inner_size` is deliberately the pair the window-state
    // plugin itself persists and restores, so the clamp reasons about the exact
    // numbers that produced the bad state. On this window the two sizes are the
    // same anyway: `titleBarStyle: "Overlay"` gives the content view the full
    // frame, so there is no title bar outside it.
    //
    // The scale factor comes with them, and an unreadable one skips the clamp
    // rather than falling back to 1.0. It is what converts the pair into
    // points, so a guess here is a wrong rect, not a coarse threshold.
    let (Ok(position), Ok(size), Ok(scale)) = (
        window.outer_position(),
        window.inner_size(),
        window.scale_factor(),
    ) else {
        eprintln!("[Tauri] Could not read the restored window geometry: skipping the clamp");
        return;
    };
    let restored = Rect::from_physical(position, size, scale);

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
        panels: monitors.iter().map(panel_points).collect(),
        primary: panel_points(&primary).work_area,
    };

    let Some(fixed) = sanitize(restored, &displays, &policy) else {
        return;
    };
    eprintln!(
        "[Tauri] Restored window geometry {}x{} at {},{} is unusable on the attached displays: \
         correcting to {}x{} at {},{} (logical points)",
        restored.width,
        restored.height,
        restored.x,
        restored.y,
        fixed.width,
        fixed.height,
        fixed.x,
        fixed.y
    );
    // Through the one placer, rather than a setter pair of its own. It applies
    // logical values, which tao passes through untouched, and it MOVES before
    // it resizes. A correction can send a window to another display. A resize
    // queued across that change is read back at the wrong scale factor
    // (ADR 0178).
    crate::app_window::place_window(&window, fixed, &format!("`{label}` back on screen"));
}

/// A monitor as the clamp sees it: its usable frame and its whole screen. Both
/// are needed, for the reason [`Panel`] gives.
///
/// Converted through THIS monitor's own scale factor, which is the one tao
/// multiplied its rects by. A neighbour's factor would put the display
/// somewhere it is not. The clamp would then judge every window against a
/// desktop that does not exist.
fn panel_points(monitor: &tauri::Monitor) -> Panel {
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    Panel {
        work_area: Rect::from_physical(area.position, area.size, scale),
        frame: Rect::from_physical(*monitor.position(), *monitor.size(), scale),
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

    /// A panel whose whole screen is usable below the menu bar: the Dock is
    /// hidden, or on an edge this fixture does not model.
    fn panel_under_a_menu_bar(x: i64, y: i64, width: i64, height: i64, menu_bar: i64) -> Panel {
        Panel {
            work_area: Rect {
                x,
                y: y + menu_bar,
                width,
                height: height - menu_bar,
            },
            frame: Rect {
                x,
                y,
                width,
                height,
            },
        }
    }

    /// The internal Retina panel alone: 1728x1117 points behind 3456x2234
    /// pixels, with a 37-point menu bar. The display the shipped clamp bug was
    /// reported on.
    fn one_panel() -> Displays {
        let panel = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        Displays {
            panels: vec![panel],
            primary: panel.work_area,
        }
    }

    /// The panel above with the Dock showing along its bottom edge. It takes 80
    /// points out of the work area and none out of the screen.
    fn one_panel_with_a_dock() -> Displays {
        let mut displays = one_panel();
        displays.panels[0].work_area.height -= 80;
        displays.primary = displays.panels[0].work_area;
        displays
    }

    /// The panel above plus an external display to its right, for the
    /// unplug case.
    fn two_panels() -> Displays {
        let mut displays = one_panel();
        let external = Rect {
            x: 1728,
            y: 0,
            width: 2560,
            height: 1440,
        };
        displays.panels.push(Panel {
            work_area: external,
            frame: external,
        });
        displays
    }

    /// The desk the placement bug was reported on: a 1x 5120x1440 ultrawide as
    /// primary, and the 2x internal panel below and right of it.
    ///
    /// In points, because that is the space macOS lays displays out in. tao
    /// reports the panel's own rect doubled, at 4050,3250, and a window on it
    /// doubled to match. Those two agree with each other and with nothing else.
    fn mixed_dpi_desk() -> Displays {
        let ultrawide = Rect {
            x: 0,
            y: 30,
            width: 5120,
            height: 1410,
        };
        let panel = Rect {
            x: 2025,
            y: 1625,
            width: 1728,
            height: 1117,
        };
        Displays {
            panels: vec![
                Panel {
                    work_area: ultrawide,
                    frame: ultrawide,
                },
                Panel {
                    work_area: panel,
                    frame: panel,
                },
            ],
            primary: ultrawide,
        }
    }

    /// `tauri.conf.json`'s 480x400 minimum and 1024x768 default.
    fn policy() -> Policy {
        Policy::from_declared(480.0, 400.0, 1024.0, 768.0)
    }

    // ── Reading a physical number back into the one space ────────────────────

    // The defect, stated. One 800x600 window at 100,200 reads as every number
    // doubled on the 2x panel and as the points themselves on the 1x
    // ultrawide. Converted through the panel's own factor the two are one rect.
    // Taken raw they are two, and applying one on the other display is what
    // handed the user a doubled window off the bottom.
    #[test]
    fn one_window_reads_the_same_on_either_panel() {
        let retina = Rect::from_physical(
            PhysicalPosition::new(200, 400),
            PhysicalSize::new(1600, 1200),
            2.0,
        );
        let ultrawide = Rect::from_physical(
            PhysicalPosition::new(100, 200),
            PhysicalSize::new(800, 600),
            1.0,
        );
        assert_eq!(retina, ultrawide);
        assert_eq!(
            retina,
            Rect {
                x: 100,
                y: 200,
                width: 800,
                height: 600,
            }
        );
    }

    // The reported bug, at the boundary that now prevents it. A window in the
    // middle of the Retina panel is reported by tao at 4450,3650 2048x1536,
    // because everything about that panel is doubled. Converted, it is still on
    // the panel, so no window born on the ultrawide can wear a doubled copy.
    #[test]
    fn a_frame_captured_on_the_retina_panel_stays_on_it() {
        let captured = Rect::from_physical(
            PhysicalPosition::new(4450, 3650),
            PhysicalSize::new(2048, 1536),
            2.0,
        );
        assert_eq!(
            captured,
            Rect {
                x: 2225,
                y: 1825,
                width: 1024,
                height: 768,
            }
        );
        assert_eq!(sanitize(captured, &mixed_dpi_desk(), &policy()), None);
    }

    // The monitor side of the same rule, and what `mixed_dpi_desk` encodes. A
    // display enters the clamp through its OWN scale factor. Read tao's raw
    // 4050,3250 instead and the desk puts the panel thousands of points from
    // where it is. Every window on it is then judged against a display that
    // does not exist.
    #[test]
    fn a_monitor_enters_the_clamp_through_its_own_scale_factor() {
        let panel = Rect::from_physical(
            PhysicalPosition::new(4050, 3250),
            PhysicalSize::new(3456, 2234),
            2.0,
        );
        assert_eq!(
            panel,
            Rect {
                x: 2025,
                y: 1625,
                width: 1728,
                height: 1117,
            }
        );
        assert_eq!(panel, mixed_dpi_desk().panels[1].work_area);
    }

    // Last net only: both callers refuse to convert without a scale they read.
    #[test]
    fn a_nonsense_scale_factor_leaves_the_numbers_as_they_came() {
        let raw = Rect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        for scale in [0.0, -2.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                Rect::from_physical(
                    PhysicalPosition::new(10, 20),
                    PhysicalSize::new(30, 40),
                    scale
                ),
                raw,
                "scale {scale}"
            );
        }
    }

    // ── What the clamp decides ───────────────────────────────────────────────

    #[test]
    fn a_healthy_rect_is_left_untouched() {
        let healthy = Rect {
            x: 200,
            y: 150,
            width: 1024,
            height: 768,
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
        // 1x1 in the middle of the screen: only the size is wrong.
        let degenerate = Rect {
            x: 500,
            y: 400,
            width: 1,
            height: 1,
        };
        let fixed = sanitize(degenerate, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1024, 768));
        // Still reachable at that position, so the position is not touched.
        assert_eq!((fixed.x, fixed.y), (500, 400));
    }

    #[test]
    fn the_reported_bug_recentres_on_the_primary() {
        // 1x1 at 1727,1085: the extreme bottom-right of the panel, saved as
        // 3454,2170 in the pixels the plugin writes.
        let shipped_bug = Rect {
            x: 1727,
            y: 1085,
            width: 1,
            height: 1,
        };
        let fixed = sanitize(shipped_bug, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1024, 768));
        assert_eq!((fixed.x, fixed.y), (352, 193));
    }

    #[test]
    fn a_window_on_an_unplugged_display_recentres_on_the_primary() {
        // Saved on the external display, which is no longer attached.
        let orphaned = Rect {
            x: 2000,
            y: 100,
            width: 1024,
            height: 768,
        };
        assert_eq!(sanitize(orphaned, &two_panels(), &policy()), None);
        let fixed = sanitize(orphaned, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1024, 768));
        assert_eq!((fixed.x, fixed.y), (352, 193));
    }

    #[test]
    fn a_partly_offscreen_window_is_nudged_back_rather_than_recentred() {
        // Hanging off the right edge with only 100 points of title bar left:
        // too little to grab, but most of the window is still on screen.
        let hanging = Rect {
            x: 1628,
            y: 250,
            width: 1024,
            height: 768,
        };
        let fixed = sanitize(hanging, &one_panel(), &policy()).expect("must be corrected");
        // Pushed just far enough left to sit inside the work area, keeping the
        // size and the vertical position the user chose.
        assert_eq!((fixed.x, fixed.y), (704, 250));
        assert_eq!((fixed.width, fixed.height), (1024, 768));
    }

    #[test]
    fn a_sliver_on_the_edge_counts_as_gone_and_recentres() {
        // One point of a full-size window left on the panel: less than a drag
        // handle's worth, so there is no neighbourhood left to nudge into.
        let sliver = Rect {
            x: 1727,
            y: 250,
            width: 1024,
            height: 768,
        };
        let fixed = sanitize(sliver, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.x, fixed.y), (352, 193));
    }

    #[test]
    fn a_title_bar_above_the_work_area_is_nudged_down() {
        // Top edge under the menu bar: horizontally fine, vertically not.
        let under_the_menu_bar = Rect {
            x: 200,
            y: 0,
            width: 1024,
            height: 768,
        };
        let fixed =
            sanitize(under_the_menu_bar, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.x, fixed.y), (200, 37));
    }

    // The tester-reported shape. The plugin records a 1024x768 window on a 2x
    // panel as 2048x1536 PHYSICAL. Restored against a 1x reading, that means
    // 2048x1536 points. Everything past the panel edge is then off screen, and
    // the grab band is reachable, so the position pass alone passed it through.
    #[test]
    fn a_doubled_frame_is_capped_to_the_panel_it_lands_on() {
        let doubled = Rect {
            x: 600,
            y: 400,
            width: 2048,
            height: 1536,
        };
        let fixed = sanitize(doubled, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1728, 1080));
        assert_eq!((fixed.x, fixed.y), (0, 37));
    }

    // The lenient half of the rule: capping is for a frame NO attached display
    // can hold. This one fits the external, so the size is the user's and it
    // survives a launch taken while the window sits on the smaller panel.
    #[test]
    fn a_frame_another_attached_display_can_hold_keeps_its_size() {
        let wide = Rect {
            x: 1850,
            y: 60,
            width: 2000,
            height: 1200,
        };
        assert_eq!(sanitize(wide, &two_panels(), &policy()), None);
        // Unplug that display and there is nowhere left to put it whole.
        let fixed = sanitize(wide, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1728, 1080));
    }

    #[test]
    fn a_window_larger_than_the_screen_is_capped_to_its_work_area() {
        let oversized = Rect {
            x: -450,
            y: -100,
            width: 2000,
            height: 1200,
        };
        let fixed = sanitize(oversized, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.x, fixed.y), (0, 37));
        // It used to keep the size and align its leading edge. That left the
        // right of the app off screen with no way to reach it.
        assert_eq!((fixed.width, fixed.height), (1728, 1080));
    }

    /// The ceiling is measured against the SCREEN, not the work area, and this
    /// is why. A user can drag a window's bottom edge under the Dock. Turning
    /// Dock auto-hide off shrinks the work area under a window already that
    /// tall. Judging the size against the work area would read both as
    /// corruption, and quietly shrink a window the user sized on purpose.
    #[test]
    fn a_window_reaching_under_the_dock_keeps_its_size() {
        let displays = one_panel_with_a_dock();
        // Taller than the 1000-point work area, inside the 1117-point screen.
        let over_the_dock = Rect {
            x: 0,
            y: 37,
            width: 1728,
            height: 1080,
        };
        assert_eq!(sanitize(over_the_dock, &displays, &policy()), None);
    }

    /// The floor wins over the ceiling. A screen under the declared minimum
    /// cannot be served at all. Capping to it would return a window the config
    /// calls unusable, and step 1 would bounce that back to the default on the
    /// next run. The two would then take turns forever.
    #[test]
    fn a_cap_never_goes_under_the_declared_minimum() {
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 300,
            height: 250,
        };
        let displays = Displays {
            panels: vec![Panel {
                work_area: tiny,
                frame: tiny,
            }],
            primary: tiny,
        };
        let fixed = sanitize(
            Rect {
                x: 0,
                y: 0,
                width: 2000,
                height: 1200,
            },
            &displays,
            &policy(),
        )
        .expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (480, 400));
        assert_eq!(sanitize(fixed, &displays, &policy()), None);
    }

    /// The reason `clamp_restored_geometry` returns early on a fullscreen
    /// window. A macOS fullscreen frame is the whole SCREEN, menu-bar strip
    /// included, so it is outside the work area at the top and the grab band
    /// reads as unreachable. The ceiling leaves it alone, since the screen holds
    /// it by definition. Nothing is wrong with it; the clamp simply has no
    /// business judging it.
    #[test]
    fn a_fullscreen_frame_reads_as_unreachable_which_is_why_it_is_skipped() {
        let fullscreen = Rect {
            x: 0,
            y: 0,
            width: 1728,
            height: 1117,
        };
        assert!(sanitize(fullscreen, &one_panel(), &policy()).is_some());
    }

    #[test]
    fn a_window_on_the_second_display_keeps_its_place() {
        let external = Rect {
            x: 1850,
            y: 50,
            width: 1024,
            height: 600,
        };
        assert_eq!(sanitize(external, &two_panels(), &policy()), None);
    }

    #[test]
    fn a_correction_is_itself_healthy_so_the_clamp_cannot_loop() {
        let broken = [
            Rect {
                x: 1727,
                y: 1085,
                width: 1,
                height: 1,
            },
            Rect {
                x: 2000,
                y: 100,
                width: 1024,
                height: 768,
            },
            Rect {
                x: 1628,
                y: 250,
                width: 1024,
                height: 768,
            },
            Rect {
                x: -450,
                y: -100,
                width: 2000,
                height: 1200,
            },
            // Bigger than the one attached panel, which the ceiling caps.
            Rect {
                x: 600,
                y: 400,
                width: 2048,
                height: 1536,
            },
            // Oversize in one axis only, and off the top in the other.
            Rect {
                x: 100,
                y: 0,
                width: 3000,
                height: 700,
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

    // The declared numbers reach the clamp unchanged, whatever panel the window
    // is on. No scale factor multiplies them any more, which is the whole point
    // of the module speaking the space the config is written in.
    #[test]
    fn the_thresholds_are_the_declared_points() {
        let policy = policy();
        assert_eq!((policy.min_width, policy.min_height), (480, 400));
        assert_eq!((policy.default_width, policy.default_height), (1024, 768));
        assert_eq!((policy.grab_width, policy.grab_height), (120, 28));
    }

    #[test]
    fn a_nonsense_config_value_leaves_no_floor_rather_than_a_wrong_one() {
        let policy = Policy::from_declared(f64::NAN, -10.0, 1024.0, 768.0);
        assert_eq!((policy.min_width, policy.min_height), (0, 0));
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
