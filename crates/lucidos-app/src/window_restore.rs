//! What frame a window may wear on the displays attached right now.
//!
//! One judgement, asked at two moments. A RESTORE asks it of a rect that came
//! off a file, and that is the whole of [`sanitize`]. A DESK CHANGE asks it of
//! a window the user is looking at, and there only the size rule applies: see
//! [`fit_to_displays`] for why the rest would be fighting them.
//!
//! It also remembers WHICH windows it corrected, because a correction is not an
//! arrangement: `window_persist` asks, so the record keeps the frame the user
//! chose until they move the window themselves (ADR 0215).
//!
//! The paragraphs below are the restore half's own story. `# Units` binds both.
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

use std::collections::BTreeMap;
use std::sync::Mutex;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Panel {
    /// tao's name for the monitor, what a [`DisplayAnchor`] matches on. On
    /// macOS it is `Monitor #<model number>`, stable across a relaunch and a
    /// change of primary. `None` leaves a frame on this monitor unanchored.
    name: Option<String>,
    /// The usable frame, menu bar and Dock excluded. Where a window is PLACED,
    /// since neither of those is somewhere a title bar can be grabbed.
    work_area: Rect,
    /// The whole screen. What bounds a RESIZE, which the Dock does not: the
    /// user can drag a window's bottom edge under it, and turning Dock
    /// auto-hide off shrinks the work area under a window already that tall.
    /// Judging a SIZE against the work area would read both as corruption.
    frame: Rect,
}

impl Panel {
    /// Is there any window at all this monitor could hold?
    ///
    /// A zero-sized frame is what a monitor reports while the desk is being
    /// reconfigured. It is the same fact as a monitor that is not there, and
    /// [`Displays::new`] drops it for that reason.
    fn holds_anything(&self) -> bool {
        self.frame.width > 0 && self.frame.height > 0
    }
}

/// The display layout the clamp judges a rect against, in logical points.
///
/// **It always holds at least one monitor that could hold a window**, which is
/// what [`Displays::new`] is for. Every question below is asked of `panels`,
/// and an EMPTY list answers each of them the most destructive way there is:
/// no screen can hold this window, its title bar is nowhere reachable, and it
/// has no home. So a desk nobody could read re-centred every window on the
/// primary, which is the one input where doing nothing is certainly right.
///
/// The fields are private so no other module can build one around that hole.
/// Inside this module the literal is still reachable, so every site here goes
/// through the constructor, tests included.
#[derive(Debug, Clone)]
pub(crate) struct Displays {
    /// Every currently-attached monitor that could hold a window.
    panels: Vec<Panel>,
    /// Work area of the primary monitor: where a window with nowhere left to be
    /// gets re-centred.
    primary: Rect,
}

impl Displays {
    /// The desk, or `None` when nothing readable was attached.
    ///
    /// Drops the monitors that could hold no window, then refuses a desk with
    /// nothing left. A caller that gets `None` leaves the geometry alone, which
    /// is what every other unreadable input in this module already does.
    ///
    /// `primary` is held to the same bar, and separately, because it is read on
    /// its own. It is where `sanitize` sends a window with nowhere left to be,
    /// and it comes from `primary_monitor` rather than from the list. A desk
    /// whose panels are fine but whose PRIMARY reads as zero-sized would
    /// re-centre that window at the origin, off every real screen.
    fn new(panels: Vec<Panel>, primary: Rect) -> Option<Self> {
        if primary.width <= 0 || primary.height <= 0 {
            return None;
        }
        let panels: Vec<Panel> = panels.into_iter().filter(Panel::holds_anything).collect();
        (!panels.is_empty()).then_some(Self { panels, primary })
    }
}

/// The display a remembered frame was captured on (ADR 0269).
///
/// macOS measures every window from the primary display's corner, and a dock
/// or an undock can change which display that is. A frame in raw global
/// coordinates then replays on the wrong display. The anchor lets a restore
/// shift the frame by however far its display has moved since.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DisplayAnchor {
    /// The monitor's name, see [`Panel::name`].
    name: String,
    /// The display's whole screen at capture time. Its size is half the
    /// identity, and its origin is what the shift is measured from.
    screen: Rect,
}

/// A workspace's frame as the session record keeps it: the global frame, and
/// the display it was on when that is known.
///
/// **The fields are private, so [`resolve`] is the only way to a frame that
/// can be placed.** A remembered frame taken raw is the defect ADR 0269 fixes.
///
/// The frame is flattened into the same four keys an older build writes, and
/// the anchor is one optional key beside them. So either build reads the
/// other's record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RememberedFrame {
    #[serde(flatten)]
    frame: Rect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display: Option<DisplayAnchor>,
}

impl RememberedFrame {
    pub(crate) fn new(frame: Rect, display: Option<DisplayAnchor>) -> Self {
        Self { frame, display }
    }

    /// A frame with no display behind it, which is what an older record holds.
    #[cfg(test)]
    pub(crate) fn unanchored(frame: Rect) -> Self {
        Self::new(frame, None)
    }
}

/// Where a live frame sits on the desk, as far as the session record cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Whereabouts {
    /// Its title bar is on no attached screen. No drag can put a window there,
    /// so the system did, and it is not an arrangement (ADR 0269).
    Orphaned,
    /// On screen, anchored to the display holding most of it. `None` when that
    /// display has no name, or the desk could not be read.
    OnScreen(Option<DisplayAnchor>),
}

/// Where `frame` sits on `displays`.
///
/// The orphan test is the loosest one a user gesture can satisfy: SOME of the
/// title strip on SOME screen. macOS will not let a drag put the title bar
/// anywhere else. So a window parked almost off an edge still counts as on
/// screen, which ADR 0173 and ADR 0204 refused to break.
///
/// The home display is measured against each whole SCREEN, like the orphan
/// test, because the anchor names a display and not its work area.
pub(crate) fn whereabouts(frame: Rect, displays: &Displays, policy: &Policy) -> Whereabouts {
    let strip = Rect {
        height: policy.grab_height.min(frame.height),
        ..frame
    };
    if !displays
        .panels
        .iter()
        .any(|panel| strip.overlap_area(&panel.frame) > 0)
    {
        return Whereabouts::Orphaned;
    }
    let home = displays
        .panels
        .iter()
        .max_by_key(|panel| frame.overlap_area(&panel.frame));
    Whereabouts::OnScreen(home.and_then(|panel| {
        Some(DisplayAnchor {
            name: panel.name.clone()?,
            screen: panel.frame,
        })
    }))
}

/// The frame to place for `remembered` on `displays`, before any clamp.
///
/// Shifted by how far the anchor display has moved, when exactly one attached
/// display matches the anchor. None means it is away, and several means
/// identical monitors at one resolution. Both give back the raw frame, and the
/// clamp judges whatever comes back.
pub(crate) fn resolve(remembered: &RememberedFrame, displays: &Displays) -> Rect {
    let frame = remembered.frame;
    let Some(anchor) = &remembered.display else {
        return frame;
    };
    let mut matches = displays.panels.iter().filter(|panel| {
        panel.name.as_deref() == Some(anchor.name.as_str())
            && panel.frame.width == anchor.screen.width
            && panel.frame.height == anchor.screen.height
    });
    match (matches.next(), matches.next()) {
        (Some(panel), None) => Rect {
            x: frame.x + panel.frame.x - anchor.screen.x,
            y: frame.y + panel.frame.y - anchor.screen.y,
            ..frame
        },
        _ => frame,
    }
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

/// The correction a LIVE window is owed now that the desk has changed, or
/// `None` to leave it exactly as it is.
///
/// Pure, so the runtime rule is testable without a second monitor.
///
/// **One gate in front of [`sanitize`], and no arithmetic of its own.** A
/// second notion of a healthy frame is how the restore path and the runtime
/// path would come to different arrangements for one window.
///
/// The gate is the SIZE rule alone. A frame no attached SCREEN can hold is a
/// shape no gesture can produce, so it is corruption wherever it comes from
/// (ADR 0193). Everything else `sanitize` asks is about a rect nobody has seen
/// yet: a size under the declared minimum, a title bar with nowhere to be
/// grabbed, a window on a display that is gone. A live window's position is
/// where the user PUT it, and ADR 0173 refused to move one parked on an edge.
///
/// So this fires on an unplug, and never on a drag. The user can carry a big
/// window onto a small display all day while the big one is still attached.
pub(crate) fn fit_to_displays(live: Rect, displays: &Displays, policy: &Policy) -> Option<Rect> {
    if a_screen_can_hold(&live, displays) {
        return None;
    }
    sanitize(live, displays, policy)
}

/// The share of the primary's work area a fresh window takes, as a fraction.
const FRESH_SHARE_NUMERATOR: i64 = 4;
const FRESH_SHARE_DENOMINATOR: i64 = 5;

/// The largest frame a fresh window takes, so a big external display does not
/// hand out a wall-sized window nobody asked for.
const FRESH_MAX_WIDTH_POINTS: i64 = 1680;
const FRESH_MAX_HEIGHT_POINTS: i64 = 1050;

/// Where a new window opened beside `source` goes: at its size, one title bar
/// down and to the right, as macOS cascades. The step is the drag handle's
/// height, which is one title bar.
///
/// A step past the edge of `source`'s work area shrinks the window to fit, so
/// it never lands exactly on top of its source. A step that starts outside the
/// work area, or a fit below the declared minimum, wraps to its top-left corner.
pub(crate) fn cascade(source: Rect, displays: &Displays, policy: &Policy) -> Rect {
    let home = home_work_area(&source, displays, policy).unwrap_or(displays.primary);
    let x = source.x + policy.grab_height;
    let y = source.y + policy.grab_height;
    let fitted = Rect {
        x,
        y,
        width: source.width.min(home.right() - x),
        height: source.height.min(home.bottom() - y),
    };
    let fits = x >= home.x
        && y >= home.y
        && fitted.width >= policy.min_width
        && fitted.height >= policy.min_height;
    if fits {
        return fitted;
    }
    Rect {
        x: home.x,
        y: home.y,
        width: source.width.min(home.width),
        height: source.height.min(home.height),
    }
}

/// Where a new window with nothing to cascade from goes: most of the primary's
/// work area, centred on it.
///
/// Sized to the screen rather than to a fixed number, so a large display and a
/// raised UI scale both get room for the panes. Never smaller than the declared
/// default, unless the work area itself is.
pub(crate) fn fresh(displays: &Displays, policy: &Policy) -> Rect {
    let work = displays.primary;
    let share = |span: i64, max: i64, default: i64| {
        (span * FRESH_SHARE_NUMERATOR / FRESH_SHARE_DENOMINATOR)
            .min(max)
            .max(default)
            .min(span)
    };
    let width = share(work.width, FRESH_MAX_WIDTH_POINTS, policy.default_width);
    let height = share(work.height, FRESH_MAX_HEIGHT_POINTS, policy.default_height);
    Rect {
        x: work.x + (work.width - width) / 2,
        y: work.y + (work.height - height) / 2,
        width,
        height,
    }
}

/// The frame each window is wearing because THIS client corrected it, by label.
///
/// A correction is not an arrangement, and `window_persist` reads this so the
/// record keeps what the user chose. See ADR 0215.
static RESCUED: Mutex<BTreeMap<String, Rect>> = Mutex::new(BTreeMap::new());

/// Record that `label` is on screen at `frame` because the clamp put it there.
///
/// Every correction site calls this, so there is no path that moves a window
/// without saying it was not the user.
fn note_rescued(label: &str, frame: Rect) {
    RESCUED.lock().unwrap().insert(label.to_string(), frame);
}

/// Is `label` still wearing the frame a correction gave it?
///
/// **The comparison IS the expiry.** Any other rect means the user has moved or
/// resized the window since. The note is dropped here, and their frame is
/// recorded from now on. Nothing has to tell our own `Moved` event from theirs,
/// which is not a thing tao reports.
///
/// A window that lands a point off what was asked reads as moved, and the
/// record takes the live frame. That is what shipped before ADR 0215, so the
/// inexact case degrades to the old behaviour rather than to something worse.
pub(crate) fn is_wearing_a_rescue(label: &str, live: Rect) -> bool {
    let mut rescued = RESCUED.lock().unwrap();
    match rescued.get(label) {
        Some(frame) if *frame == live => true,
        Some(_) => {
            rescued.remove(label);
            false
        }
        None => false,
    }
}

/// Drop `label`'s note, because the window is gone.
///
/// `main` is hidden rather than closed, and comes back on its workspace. A note
/// outliving its window would hold the record against a later, unrelated frame.
pub(crate) fn forget_rescue(label: &str) {
    RESCUED.lock().unwrap().remove(label);
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

/// What a rect has to be judged against: the declared floors, and the desk as
/// it is right now. `None` when any of it is unreadable, which every caller
/// treats as "leave the geometry alone".
///
/// One reader, so judging a rect the client is ABOUT to write and judging one a
/// window already wears cannot drift apart.
///
/// The desk comes off the app rather than a window, because a restore has to
/// resolve a frame before the window it is for exists.
fn policy_and_displays(app: &tauri::AppHandle, label: &str) -> Option<(Policy, Displays)> {
    let Some(config) = policy_config(&app.config().app.windows, label).cloned() else {
        eprintln!("[Tauri] No window config to judge `{label}` against: skipping the clamp");
        return None;
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
    let Ok(monitors) = app.available_monitors() else {
        eprintln!("[Tauri] Could not enumerate monitors: skipping the restore clamp");
        return None;
    };
    let primary = app
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| monitors.first().cloned());
    let Some(primary) = primary else {
        eprintln!("[Tauri] No monitor to place the window on: skipping the restore clamp");
        return None;
    };
    let Some(displays) = Displays::new(
        monitors.iter().map(panel_points).collect(),
        panel_points(&primary).work_area,
    ) else {
        eprintln!("[Tauri] No monitor that could hold a window: skipping the clamp");
        return None;
    };
    Some((policy, displays))
}

/// Say what a correction did, in the one wording both callers use.
///
/// It names the DESK as well as the two frames. With the frames alone, which
/// rule fired has to be derived from the arithmetic. The answer is often
/// "against a desk that was not there".
fn log_correction(what: &str, before: Rect, after: Rect, displays: &Displays) {
    let primary = displays.primary;
    eprintln!(
        "[Tauri] {what} {}x{} at {},{} is unusable on the attached displays: \
         correcting to {}x{} at {},{} (logical points, judged against {} panel(s), \
         primary work area {}x{} at {},{})",
        before.width,
        before.height,
        before.x,
        before.y,
        after.width,
        after.height,
        after.x,
        after.y,
        displays.panels.len(),
        primary.width,
        primary.height,
        primary.x,
        primary.y
    );
}

/// [`resolve`], saying so when the anchor moved the frame.
///
/// The line names the display, because a window coming back somewhere
/// unexpected is otherwise a puzzle in arithmetic (ADR 0269).
fn resolve_logged(remembered: &RememberedFrame, displays: &Displays, label: &str) -> Rect {
    let resolved = resolve(remembered, displays);
    if let (Some(anchor), true) = (&remembered.display, resolved != remembered.frame) {
        let before = remembered.frame;
        eprintln!(
            "[Tauri] `{label}` was left on {}, which has moved since: its frame \
             {}x{} at {},{} restores at {},{} (logical points)",
            anchor.name, before.width, before.height, before.x, before.y, resolved.x, resolved.y
        );
    }
    resolved
}

/// The frame to BUILD a window at, for a workspace the record remembers.
///
/// Resolved against the desk and not judged, because the window does not exist
/// yet. [`clamp_restored_geometry`] judges it once the builder has placed it.
/// An unreadable desk gives back the raw frame.
pub(crate) fn frame_to_build(
    app: &tauri::AppHandle,
    label: &str,
    remembered: &RememberedFrame,
) -> Rect {
    match policy_and_displays(app, label) {
        Some((_, displays)) => resolve_logged(remembered, &displays, label),
        None => remembered.frame,
    }
}

/// The frame a new window cascades from, when `window` can be one.
///
/// `None` for a fullscreen window, whose frame is the whole screen and no
/// ordinary window should copy, and for a frame that cannot be read.
pub(crate) fn cascade_source(window: &tauri::Window) -> Option<Rect> {
    if window.is_fullscreen().unwrap_or(false) {
        return None;
    }
    live_frame(window)
}

/// The frame to BUILD a new window at: cascaded from `source` when there is
/// one, else [`fresh`]. `None` only when the desk is unreadable, and the caller
/// falls back to the declared default.
pub(crate) fn new_window_frame(
    app: &tauri::AppHandle,
    label: &str,
    source: Option<Rect>,
) -> Option<Rect> {
    let (policy, displays) = policy_and_displays(app, label)?;
    Some(match source {
        Some(frame) => cascade(frame, &displays, &policy),
        None => fresh(&displays, &policy),
    })
}

/// Where a live window's `frame` sits on the desk, for the session capture.
///
/// An unreadable desk answers "on screen, unanchored". The capture then records
/// the frame unanchored rather than holding the old one on a guess.
pub(crate) fn whereabouts_now(app: &tauri::AppHandle, label: &str, frame: Rect) -> Whereabouts {
    match policy_and_displays(app, label) {
        Some((policy, displays)) => whereabouts(frame, &displays, &policy),
        None => Whereabouts::OnScreen(None),
    }
}

/// The frame to actually place a window at, given the one a record names.
///
/// The same judgement [`clamp_restored_geometry`] makes, taken BEFORE the frame
/// is written rather than after. A placement is deferred to tao's main queue. A
/// clamp issued behind one therefore reads the geometry the window still has,
/// and judges the rect it is about to lose. That is ADR 0193's own trap, one
/// step over: there the stale read was the window's birth size, here it would
/// be the window-state plugin's.
///
/// The frame is resolved against its display first (ADR 0269), and the clamp
/// judges the result. A healthy rect comes back unchanged, so the caller can
/// place the result unconditionally.
pub(crate) fn sanitized_frame(
    app: &tauri::AppHandle,
    label: &str,
    remembered: &RememberedFrame,
) -> Rect {
    let Some((policy, displays)) = policy_and_displays(app, label) else {
        return remembered.frame;
    };
    let frame = resolve_logged(remembered, &displays, label);
    match sanitize(frame, &displays, &policy) {
        Some(fixed) => {
            log_correction(
                &format!("The frame `{label}` is owed is"),
                frame,
                fixed,
                &displays,
            );
            // The caller places this rect, so the window is about to wear a
            // frame nobody chose. The record must keep the one it has, per
            // ADR 0215.
            note_rescued(label, fixed);
            fixed
        }
        None => frame,
    }
}

/// Read a window's restored geometry, sanity-check it, and correct it in place
/// if it is unusable.
///
/// For a window whose geometry the client did NOT choose. Every caller runs
/// just before it reaches the screen, which is the rule (ADR 0193): the startup
/// show when no frame is remembered, `reopen_client`, `open_app_window`, and
/// `front_window` for one brought forward that is not up yet. NOT `setup`,
/// where tao's deferred setters have not landed and the read is of the geometry
/// the window was born at.
///
/// Where the client DOES choose the frame, [`sanitized_frame`] judges it first
/// and this never runs. Clamping after a placement would read the geometry the
/// placement has not replaced yet.
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
    clamp_geometry(app, label, "Restored window geometry", sanitize);
}

/// Read a LIVE window's geometry, ask whether the desk can still hold it, and
/// shrink it onto a display that can if not.
///
/// The runtime twin of [`clamp_restored_geometry`], and the only path in the
/// client that corrects a window already on screen. `window_desk` is its one
/// caller, once per display-configuration change.
///
/// [`fit_to_displays`] is what makes that safe: the size rule alone, so a
/// window the user sized, moved or parked is untouched while a display can hold
/// it.
pub(crate) fn clamp_live_geometry(app: &tauri::AppHandle, label: &str) {
    clamp_geometry(app, label, "Live window geometry", fit_to_displays);
}

/// The body both clamps share: read the window's real geometry in points, ask
/// `decide` about it, and place the answer.
///
/// One reader, so the two moments cannot come to different arrangements for one
/// window. They differ in the QUESTION alone, which is the `decide` argument.
fn clamp_geometry(
    app: &tauri::AppHandle,
    label: &str,
    what: &str,
    decide: fn(Rect, &Displays, &Policy) -> Option<Rect>,
) {
    let Some(window) = app.get_window(label) else {
        return;
    };
    // A fullscreen window is the one case where being outside every work area
    // is CORRECT. macOS gives it the whole screen, menu-bar strip included, so
    // its title strip fails the reachability check by construction (pinned by
    // `a_fullscreen_frame_reads_as_unreachable_which_is_why_it_is_skipped`).
    // macOS owns that frame, and the AppKit transition is asynchronous, so a
    // correction would land mid-flight. tao records the flag synchronously
    // inside `set_fullscreen`, so this reads true from the moment it was asked
    // for rather than when the animation ends.
    //
    // MAXIMIZED is deliberately NOT skipped: a genuinely maximized window's rect
    // IS the work area, which `sanitize` passes through untouched, while a
    // window that merely CLAIMS to be maximized while sitting in a corner is
    // exactly the state this exists to fix.
    if window.is_fullscreen().unwrap_or(false) {
        return;
    }
    let Some((policy, displays)) = policy_and_displays(app, label) else {
        return;
    };

    let Some(worn) = live_frame(&window) else {
        eprintln!("[Tauri] Could not read the geometry of `{label}`: skipping the clamp");
        return;
    };

    let Some(fixed) = decide(worn, &displays, &policy) else {
        return;
    };
    log_correction(what, worn, fixed, &displays);
    // Noted BEFORE the placement, so the note can never lag the frame it
    // describes. Both run in this one main-thread turn, so nothing reads the
    // window in between. This frame is not the user's, and ADR 0215 keeps it
    // out of the record until they move the window themselves.
    note_rescued(label, fixed);
    // Through the one placer, rather than a setter pair of its own. It applies
    // logical values, which tao passes through untouched, and it MOVES before
    // it resizes. A correction can send a window to another display. A resize
    // queued across that change is read back at the wrong scale factor
    // (ADR 0178).
    crate::app_window::place_window(&window, fixed, &format!("`{label}` back on screen"));
}

/// The frame `window` is wearing right now, in points, or `None` when any part
/// of it could not be read.
///
/// **The one reader of a live window's FRAME**, so the clamp, the session
/// capture and the rescue check all reason about one set of numbers. Other
/// readers take a window's size for their own purposes (`refit_webview`,
/// `panel_preview::title_bar_gap`); none of them produces a frame.
///
/// Position plus CONTENT size, which is the pair the window-state plugin
/// persists and restores. The size comes from `app_window::window_content_size`
/// and never from `inner_size`, which answers with the PAGE on macOS. The clamp
/// spent its life judging the webview's frame and calling it the window's
/// (ADR 0202).
///
/// The scale factor comes with them, and an unreadable one gives `None` rather
/// than falling back to 1.0. It is what converts the pair into points, so a
/// guess here is a wrong rect rather than a coarse one.
///
/// By window, not webview window, per ADR 0140.
pub(crate) fn live_frame(window: &tauri::Window) -> Option<Rect> {
    let (Ok(position), Ok(size), Ok(scale)) = (
        window.outer_position(),
        crate::app_window::window_content_size(window),
        window.scale_factor(),
    ) else {
        return None;
    };
    Some(Rect::from_physical(position, size, scale))
}

/// A monitor as the clamp sees it: its name, its usable frame and its whole
/// screen. Both rects are needed, for the reason [`Panel`] gives.
///
/// Converted through THIS monitor's own scale factor, which is the one tao
/// multiplied its rects by. A neighbour's factor would put the display
/// somewhere it is not. The clamp would then judge every window against a
/// desktop that does not exist.
fn panel_points(monitor: &tauri::Monitor) -> Panel {
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    Panel {
        name: monitor.name().cloned(),
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
            name: None,
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

    /// A desk a fixture declares, through the one constructor production uses.
    fn desk(panels: Vec<Panel>, primary: Rect) -> Displays {
        Displays::new(panels, primary).expect("a fixture desk must hold a window")
    }

    /// One monitor whose whole screen is its work area.
    fn plain_panel(rect: Rect) -> Panel {
        Panel {
            name: None,
            work_area: rect,
            frame: rect,
        }
    }

    /// The internal Retina panel alone: 1728x1117 points behind 3456x2234
    /// pixels, with a 37-point menu bar. The display the shipped clamp bug was
    /// reported on.
    fn one_panel() -> Displays {
        let panel = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        desk(vec![panel.clone()], panel.work_area)
    }

    /// The panel above with the Dock showing along its bottom edge. It takes 80
    /// points out of the work area and none out of the screen.
    fn one_panel_with_a_dock() -> Displays {
        let mut panel = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        panel.work_area.height -= 80;
        desk(vec![panel.clone()], panel.work_area)
    }

    /// The panel above plus an external display to its right, for the
    /// unplug case.
    fn two_panels() -> Displays {
        let panel = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        let external = Rect {
            x: 1728,
            y: 0,
            width: 2560,
            height: 1440,
        };
        desk(vec![panel.clone(), plain_panel(external)], panel.work_area)
    }

    /// A 1x laptop panel with a 1x external display beside it: one desk, ONE
    /// scale factor.
    ///
    /// The variant of the reported bug that fires no `ScaleFactorChanged` at
    /// all, because tao emits that event only when the backing factor actually
    /// changes. Nothing here can tell: the decision is in points and never
    /// reads a scale factor. That is exactly why it covers the variant.
    fn two_one_x_panels() -> Displays {
        let laptop = panel_under_a_menu_bar(0, 0, 1440, 900, 25);
        let external = Rect {
            x: 1440,
            y: 0,
            width: 2560,
            height: 1440,
        };
        desk(
            vec![laptop.clone(), plain_panel(external)],
            laptop.work_area,
        )
    }

    /// The desk above after the external display is unplugged. Derived from it,
    /// so the two cannot drift into describing different laptops.
    fn the_laptop_alone() -> Displays {
        let laptop = two_one_x_panels().panels[0].clone();
        desk(vec![laptop.clone()], laptop.work_area)
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
        desk(vec![plain_panel(ultrawide), plain_panel(panel)], ultrawide)
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

    // ── Which frames are the client's rather than the user's (ADR 0215) ──────

    /// A label no other test uses. `RESCUED` is process-wide, and the test
    /// binary runs its threads in one process.
    fn a_rescued_frame() -> Rect {
        Rect {
            x: 643,
            y: 30,
            width: 1728,
            height: 1084,
        }
    }

    #[test]
    fn a_window_nothing_corrected_is_wearing_no_rescue() {
        assert!(!is_wearing_a_rescue(
            "rescue-test-untouched",
            a_rescued_frame()
        ));
    }

    #[test]
    fn a_window_still_at_the_corrected_frame_is_wearing_the_rescue() {
        note_rescued("rescue-test-held", a_rescued_frame());
        assert!(is_wearing_a_rescue("rescue-test-held", a_rescued_frame()));
        // Asked twice, because both the plugin gate and the session capture ask.
        assert!(is_wearing_a_rescue("rescue-test-held", a_rescued_frame()));
        forget_rescue("rescue-test-held");
    }

    /// The expiry, and the reason it needs no timer. Any frame but the one we
    /// placed is the user having moved or resized the window.
    #[test]
    fn a_frame_the_user_changed_ends_the_rescue_for_good() {
        note_rescued("rescue-test-moved", a_rescued_frame());
        let moved = Rect {
            x: 200,
            ..a_rescued_frame()
        };
        assert!(!is_wearing_a_rescue("rescue-test-moved", moved));
        // Dropped, so putting the window back where the clamp had it does not
        // revive a note the user already spent.
        assert!(!is_wearing_a_rescue("rescue-test-moved", a_rescued_frame()));
    }

    #[test]
    fn forgetting_a_window_drops_its_rescue() {
        note_rescued("rescue-test-closed", a_rescued_frame());
        forget_rescue("rescue-test-closed");
        assert!(!is_wearing_a_rescue(
            "rescue-test-closed",
            a_rescued_frame()
        ));
    }

    // ── What counts as a desk at all ─────────────────────────────────────────

    /// The defect this constructor exists for. `available_monitors` can answer
    /// `Ok` with nothing in it, while `primary_monitor` still names a display.
    /// Every question the clamp asks is asked of the panel list. An empty one
    /// therefore says three things at once, about every window: no screen can
    /// hold it, no title bar is reachable, and it has no home.
    #[test]
    fn a_desk_with_no_panels_is_not_a_desk() {
        let primary = one_panel().primary;
        assert!(Displays::new(Vec::new(), primary).is_none());
    }

    /// A monitor reporting a zero-sized frame is the same fact as one that is
    /// not attached: no window fits on it. Left in the list it drags every
    /// predicate to the empty list's answer, while looking like a real desk.
    #[test]
    fn a_desk_of_zero_sized_panels_is_not_a_desk() {
        let nothing = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let primary = one_panel().primary;
        assert!(Displays::new(vec![plain_panel(nothing)], primary).is_none());
    }

    #[test]
    fn a_zero_sized_panel_is_dropped_from_a_desk_that_has_a_real_one() {
        let real = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        let nothing = plain_panel(Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
        let displays = Displays::new(vec![nothing, real.clone()], real.work_area)
            .expect("the real panel remains");
        assert_eq!(displays.panels, vec![real]);
    }

    /// The primary is read on its own, so it needs its own guard. `sanitize`
    /// re-centres a window with nowhere left to be on it, and a zero-sized one
    /// puts that window at the origin: off every real screen, and recorded as
    /// a rescue the workspace is then held behind.
    #[test]
    fn a_desk_whose_primary_could_hold_nothing_is_not_a_desk() {
        let real = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        let nothing = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        assert!(Displays::new(vec![real], nothing).is_none());
    }

    #[test]
    fn a_healthy_list_reaches_the_clamp_unchanged() {
        let laptop = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        let external = plain_panel(Rect {
            x: 1728,
            y: 0,
            width: 2560,
            height: 1440,
        });
        let displays = Displays::new(vec![laptop.clone(), external.clone()], laptop.work_area)
            .expect("a desk");
        assert_eq!(displays.panels, vec![laptop.clone(), external]);
        assert_eq!(displays.primary, laptop.work_area);
    }

    /// The primary the shipped correction's arithmetic names. Re-centring
    /// 1280 wide on it lands at `0 + (1728 - 1280) / 2 = 224`, the logged x.
    fn reported_primary() -> Rect {
        Rect {
            x: 0,
            y: 33,
            width: 1728,
            height: 1084,
        }
    }

    /// The correction that shipped, in the numbers the client logged:
    /// `1280x1084 at 0,33` re-centred to `224,33` on the 16 inch panel.
    ///
    /// Read against that panel the window fits, so BOTH clamps owe it nothing.
    /// The shipped correction kept the SIZE. So step 2 found a home work area
    /// big enough to hold it, and a work area sits inside its own screen. That
    /// means `a_screen_can_hold` had to agree and no correction was owed. The
    /// only reading left is a desk with no panels, now refused.
    #[test]
    fn the_window_the_client_moved_needed_no_correction_at_all() {
        let panel = panel_under_a_menu_bar(0, 0, 1728, 1117, 33);
        let displays = desk(vec![panel.clone()], panel.work_area);
        let reported = Rect {
            x: 0,
            y: 33,
            width: 1280,
            height: 1084,
        };
        assert_eq!(displays.primary, reported_primary());
        assert_eq!(fit_to_displays(reported, &displays, &policy()), None);
        assert_eq!(sanitize(reported, &displays, &policy()), None);
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
        let displays = desk(vec![plain_panel(tiny)], tiny);
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

    // ── What a DESK CHANGE decides, about a window the user is looking at ────

    // The report. A window sized on the external display, carried to the
    // built-in panel by unplugging the external one. Nothing in the client saw
    // this before: every clamp call site is a window creation or a show.
    #[test]
    fn an_undocked_window_is_capped_to_the_panel_that_is_left() {
        let sized_on_the_external = Rect {
            x: 1850,
            y: 60,
            width: 2000,
            height: 1200,
        };
        let fixed = fit_to_displays(sized_on_the_external, &one_panel(), &policy())
            .expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1728, 1080));
        assert_eq!((fixed.x, fixed.y), (0, 37));
    }

    // The variant that fires no `ScaleFactorChanged`, since both displays
    // report the same backing factor. One decision covers both, because the
    // decision is in points and never reads a factor.
    #[test]
    fn an_undock_between_two_one_x_displays_is_the_same_decision() {
        let sized_on_the_external = Rect {
            x: 1500,
            y: 100,
            width: 2200,
            height: 1300,
        };
        let displays = two_one_x_panels();
        assert_eq!(
            fit_to_displays(sized_on_the_external, &displays, &policy()),
            None
        );
        let fixed = fit_to_displays(sized_on_the_external, &the_laptop_alone(), &policy())
            .expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1440, 875));
        assert_eq!((fixed.x, fixed.y), (0, 25));
    }

    // The mixed-DPI desk, where the two panels agree with each other and with
    // nothing else in physical pixels. Unplugging the ultrawide leaves a window
    // no remaining screen can hold.
    #[test]
    fn a_window_sized_for_the_ultrawide_is_capped_to_the_retina_panel() {
        let sized_on_the_ultrawide = Rect {
            x: 200,
            y: 100,
            width: 3000,
            height: 1200,
        };
        assert_eq!(
            fit_to_displays(sized_on_the_ultrawide, &mixed_dpi_desk(), &policy()),
            None
        );
        let panel = mixed_dpi_desk().panels[1].clone();
        let retina_alone = desk(vec![panel.clone()], panel.work_area);
        let fixed = fit_to_displays(sized_on_the_ultrawide, &retina_alone, &policy())
            .expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1728, 1117));
        assert_eq!((fixed.x, fixed.y), (2025, 1625));
    }

    // The lenient rule, at runtime. ADR 0193 weighed it and chose it: a window
    // sized for a monitor that is merely away comes back whole. Dragging one
    // onto the small panel is therefore not a correction either, which is what
    // keeps the pass off a live drag between displays.
    #[test]
    fn a_window_another_attached_display_can_hold_is_left_alone() {
        let big = Rect {
            x: 1850,
            y: 60,
            width: 2000,
            height: 1200,
        };
        let displays = two_panels();
        assert_eq!(fit_to_displays(big, &displays, &policy()), None);
        let dragged_onto_the_small_panel = Rect {
            x: 100,
            y: 100,
            ..big
        };
        assert_eq!(
            fit_to_displays(dragged_onto_the_small_panel, &displays, &policy()),
            None
        );
    }

    // The gate is the size rule ALONE, and these two are why. A rect off a file
    // with no grabbable title bar is unusable, so the restore clamp nudges it.
    // The same rect on a live window is where the user dragged it.
    #[test]
    fn a_window_the_user_parked_half_off_screen_is_left_alone() {
        let hanging = Rect {
            x: 1628,
            y: 250,
            width: 1024,
            height: 768,
        };
        let displays = one_panel();
        assert!(sanitize(hanging, &displays, &policy()).is_some());
        assert_eq!(fit_to_displays(hanging, &displays, &policy()), None);
    }

    // The declared floor is a restore rule too. `open_app_window` applies
    // `min_inner_size` to every window it builds, so no gesture reaches this
    // shape; correcting it would only be the pass inventing work.
    #[test]
    fn a_live_window_under_the_declared_minimum_is_left_alone() {
        let degenerate = Rect {
            x: 500,
            y: 400,
            width: 1,
            height: 1,
        };
        let displays = one_panel();
        assert!(sanitize(degenerate, &displays, &policy()).is_some());
        assert_eq!(fit_to_displays(degenerate, &displays, &policy()), None);
    }

    // The second lock on the pass fighting itself. The first is that our own
    // placement cannot post a screen-parameters change, so nothing re-arms the
    // pass. This one is that a correction leaves a rect the pass has no more to
    // say about.
    #[test]
    fn a_desk_change_correction_is_itself_healthy_so_the_pass_cannot_loop() {
        let cases = [
            (
                Rect {
                    x: 1850,
                    y: 60,
                    width: 2000,
                    height: 1200,
                },
                one_panel(),
            ),
            (
                Rect {
                    x: 1500,
                    y: 100,
                    width: 2200,
                    height: 1300,
                },
                the_laptop_alone(),
            ),
            // Oversize in one axis only.
            (
                Rect {
                    x: 100,
                    y: 200,
                    width: 3000,
                    height: 700,
                },
                one_panel(),
            ),
        ];
        for (rect, displays) in cases {
            let fixed = fit_to_displays(rect, &displays, &policy()).expect("must be corrected");
            assert_eq!(
                fit_to_displays(fixed, &displays, &policy()),
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

    // ── A remembered frame is anchored to its display (ADR 0269) ─────────────

    const EXTERNAL: &str = "Monitor #41003";
    const BUILT_IN: &str = "Monitor #41216";

    fn named(name: &str, panel: Panel) -> Panel {
        Panel {
            name: Some(name.to_string()),
            ..panel
        }
    }

    /// The reported desk, docked: a 1x ultrawide as primary, and the 2x
    /// built-in below it and to the right.
    fn docked_desk() -> Displays {
        let external = named(EXTERNAL, panel_under_a_menu_bar(0, 0, 5120, 1440, 30));
        let built_in = named(BUILT_IN, panel_under_a_menu_bar(1763, 1440, 1728, 1117, 32));
        let primary = external.work_area;
        desk(vec![external, built_in], primary)
    }

    /// The same laptop undocked. The built-in is now the primary, at 0,0, so
    /// every coordinate on it has moved by (1763, 1440).
    fn undocked_desk() -> Displays {
        let built_in = named(BUILT_IN, panel_under_a_menu_bar(0, 0, 1728, 1117, 32));
        let primary = built_in.work_area;
        desk(vec![built_in], primary)
    }

    /// What the capture records for a window wearing `frame` on `displays`.
    fn captured(frame: Rect, displays: &Displays) -> RememberedFrame {
        match whereabouts(frame, displays, &policy()) {
            Whereabouts::OnScreen(anchor) => RememberedFrame::new(frame, anchor),
            Whereabouts::Orphaned => panic!("{frame:?} is on screen in this fixture"),
        }
    }

    fn at(x: i64, y: i64, width: i64, height: i64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    // The reported defect. A window left under the built-in's menu bar while
    // undocked was replayed raw after docking, which is the external display.
    #[test]
    fn a_frame_left_on_the_built_in_undocked_comes_back_on_it_docked() {
        let remembered = captured(at(0, 33, 1280, 1084), &undocked_desk());
        assert_eq!(
            resolve(&remembered, &docked_desk()),
            at(1763, 1473, 1280, 1084)
        );
    }

    #[test]
    fn a_frame_left_on_the_built_in_docked_comes_back_on_it_undocked() {
        let remembered = captured(at(1763, 1473, 1728, 1084), &docked_desk());
        assert_eq!(
            resolve(&remembered, &undocked_desk()),
            at(0, 33, 1728, 1084)
        );
    }

    // Nothing moved, so nothing shifts. The ordinary relaunch.
    #[test]
    fn a_frame_on_a_display_that_has_not_moved_comes_back_unchanged() {
        let frame = at(643, 132, 1728, 1084);
        assert_eq!(
            resolve(&captured(frame, &docked_desk()), &docked_desk()),
            frame
        );
    }

    // The external is away. The raw frame goes to the clamp, as it always did,
    // and the clamp is what rescues it onto the built-in.
    #[test]
    fn a_frame_whose_display_is_away_comes_back_raw() {
        let frame = at(643, 132, 1728, 1084);
        let remembered = captured(frame, &docked_desk());
        assert_eq!(resolve(&remembered, &undocked_desk()), frame);
    }

    // Two identical monitors at one resolution. Guessing could land the window
    // on the wrong one, so neither is chosen.
    #[test]
    fn an_anchor_two_displays_match_comes_back_raw() {
        let left = named(EXTERNAL, plain_panel(at(0, 0, 2560, 1440)));
        let right = named(EXTERNAL, plain_panel(at(2560, 0, 2560, 1440)));
        let primary = left.work_area;
        let twins = desk(vec![left, right], primary);
        let frame = at(100, 100, 1200, 800);
        let remembered = RememberedFrame::new(
            frame,
            Some(DisplayAnchor {
                name: EXTERNAL.to_string(),
                screen: at(500, 0, 2560, 1440),
            }),
        );
        assert_eq!(resolve(&remembered, &twins), frame);
    }

    // The same model at a different resolution is a different display as far
    // as a frame is concerned.
    #[test]
    fn an_anchor_whose_display_changed_size_comes_back_raw() {
        let frame = at(1763, 1473, 1280, 1084);
        let remembered = RememberedFrame::new(
            frame,
            Some(DisplayAnchor {
                name: BUILT_IN.to_string(),
                screen: at(1763, 1440, 1512, 982),
            }),
        );
        assert_eq!(resolve(&remembered, &undocked_desk()), frame);
    }

    #[test]
    fn an_unanchored_frame_comes_back_exactly_as_recorded() {
        let frame = at(0, 356, 1280, 1084);
        let remembered = RememberedFrame::unanchored(frame);
        assert_eq!(resolve(&remembered, &docked_desk()), frame);
        assert_eq!(resolve(&remembered, &undocked_desk()), frame);
    }

    // The frame the reported launch restored, as the unplug left it: an
    // external window at 643,132, read against the built-in alone.
    #[test]
    fn a_frame_above_every_screen_is_orphaned() {
        let frame = at(643, -191, 1728, 1084);
        assert_eq!(
            whereabouts(frame, &undocked_desk(), &policy()),
            Whereabouts::Orphaned
        );
        assert_eq!(
            whereabouts(frame, &docked_desk(), &policy()),
            Whereabouts::Orphaned
        );
    }

    // ADR 0173 and ADR 0204 refused to move a window parked on an edge. A
    // sliver of title bar on screen is enough to be the user's arrangement.
    #[test]
    fn a_window_parked_almost_off_an_edge_is_still_on_screen() {
        let frame = at(5100, 400, 1200, 800);
        assert_eq!(
            whereabouts(frame, &docked_desk(), &policy()),
            Whereabouts::OnScreen(Some(DisplayAnchor {
                name: EXTERNAL.to_string(),
                screen: at(0, 0, 5120, 1440),
            }))
        );
    }

    // A window straddling both displays belongs to the one holding more of it.
    #[test]
    fn a_straddling_window_is_anchored_to_the_display_holding_most_of_it() {
        let frame = at(2000, 1300, 1200, 800);
        assert_eq!(
            whereabouts(frame, &docked_desk(), &policy()),
            Whereabouts::OnScreen(Some(DisplayAnchor {
                name: BUILT_IN.to_string(),
                screen: at(1763, 1440, 1728, 1117),
            }))
        );
    }

    // A monitor tao could not name gives nothing to match on later, so the
    // frame is recorded as an older build would record it.
    #[test]
    fn a_frame_on_an_unnamed_display_is_on_screen_and_unanchored() {
        assert_eq!(
            whereabouts(at(100, 100, 1200, 800), &one_panel(), &policy()),
            Whereabouts::OnScreen(None)
        );
    }

    // The record keeps the four keys an older build reads, and the anchor is
    // one key beside them. A rollback must read every frame this build writes.
    #[test]
    fn an_anchored_frame_is_readable_by_an_older_build() {
        let remembered = captured(at(1763, 1473, 1728, 1084), &docked_desk());
        let json = serde_json::to_string(&remembered).expect("serialize");
        let as_older_build: Rect = serde_json::from_str(&json).expect("an older build reads it");
        assert_eq!(as_older_build, at(1763, 1473, 1728, 1084));
        let back: RememberedFrame = serde_json::from_str(&json).expect("this build reads it");
        assert_eq!(back, remembered);
    }

    // An unanchored frame writes exactly the shape an older build wrote, so a
    // record no display could be read for is byte-for-byte the old one.
    #[test]
    fn an_unanchored_frame_writes_the_old_shape() {
        let json = serde_json::to_value(RememberedFrame::unanchored(at(1, 2, 1200, 800)))
            .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({"x": 1, "y": 2, "width": 1200, "height": 800})
        );
    }

    // ── Where a new window goes ──────────────────────────────────────────────

    // The macOS cascade: the source's size, one title bar down and to the
    // right.
    #[test]
    fn a_new_window_cascades_from_its_source_at_the_same_size() {
        assert_eq!(
            cascade(at(100, 137, 1400, 900), &one_panel(), &policy()),
            at(128, 165, 1400, 900)
        );
    }

    // A step past the bottom edge shrinks the window to fit rather than
    // letting it hang off the screen.
    #[test]
    fn a_cascade_off_the_bottom_shrinks_to_fit() {
        let work = one_panel().primary;
        let source = at(200, work.bottom() - 900, 1400, 900);
        assert_eq!(
            cascade(source, &one_panel(), &policy()),
            at(228, 245, 1400, 872)
        );
    }

    // A window filling the work area, or tiled to its left half, still gets a
    // visible step. Wrapping to the top-left would land exactly on top of it.
    #[test]
    fn a_cascade_from_a_window_filling_the_work_area_steps_and_shrinks() {
        let work = one_panel().primary;
        assert_eq!(
            cascade(work, &one_panel(), &policy()),
            at(28, 65, 1700, 1052)
        );
        let left_half = Rect {
            width: work.width / 2,
            ..work
        };
        assert_eq!(
            cascade(left_half, &one_panel(), &policy()),
            at(28, 65, 864, 1052)
        );
    }

    // Shrinking stops at the declared minimum. Past it, the cascade wraps to
    // the top-left corner, so repeated New Window never walks off the screen.
    #[test]
    fn a_cascade_that_cannot_fit_at_the_minimum_wraps_to_the_top_left() {
        let work = one_panel().primary;
        let source = at(work.right() - 480, work.bottom() - 400, 480, 400);
        assert_eq!(
            cascade(source, &one_panel(), &policy()),
            at(work.x, work.y, 480, 400)
        );
    }

    #[test]
    fn a_cascade_stays_on_the_display_of_its_source() {
        let source = at(1800, 100, 1600, 1000);
        assert_eq!(
            cascade(source, &two_panels(), &policy()),
            at(1828, 128, 1600, 1000)
        );
    }

    // The reported window: 1024x768 on the Retina panel at 125% UI scale left
    // the page about 820 points wide for three panes. The fresh window takes
    // most of the work area instead.
    #[test]
    fn a_fresh_window_takes_most_of_the_work_area_centred() {
        let work = one_panel().primary;
        let fresh = fresh(&one_panel(), &policy());
        assert_eq!((fresh.width, fresh.height), (1382, 864));
        assert_eq!(fresh.x, work.x + (work.width - fresh.width) / 2);
        assert_eq!(fresh.y, work.y + (work.height - fresh.height) / 2);
    }

    #[test]
    fn a_fresh_window_on_a_large_display_is_capped() {
        let big = plain_panel(at(0, 0, 3008, 1692));
        let fresh = fresh(&desk(vec![big.clone()], big.work_area), &policy());
        assert_eq!(
            (fresh.width, fresh.height),
            (FRESH_MAX_WIDTH_POINTS, FRESH_MAX_HEIGHT_POINTS)
        );
    }

    // Never below the declared default, unless the screen itself is smaller.
    #[test]
    fn a_fresh_window_is_at_least_the_declared_default_that_fits() {
        let small = plain_panel(at(0, 0, 1152, 720));
        let fresh = fresh(&desk(vec![small.clone()], small.work_area), &policy());
        assert_eq!((fresh.width, fresh.height), (1024, 720));
    }
}
