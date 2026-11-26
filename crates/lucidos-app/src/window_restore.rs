use std::collections::BTreeMap;
use std::sync::Mutex;
use tauri::{Manager, PhysicalPosition, PhysicalSize};

const GRAB_HEIGHT_POINTS: f64 = 28.0;

const GRAB_WIDTH_POINTS: f64 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    pub(crate) fn from_physical(
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
        scale: f64,
    ) -> Self {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Panel {
    name: Option<String>,
    work_area: Rect,
    frame: Rect,
}

impl Panel {
    fn holds_anything(&self) -> bool {
        self.frame.width > 0 && self.frame.height > 0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Displays {
    panels: Vec<Panel>,
    primary: Rect,
}

impl Displays {
    fn new(panels: Vec<Panel>, primary: Rect) -> Option<Self> {
        if primary.width <= 0 || primary.height <= 0 {
            return None;
        }
        let panels: Vec<Panel> = panels.into_iter().filter(Panel::holds_anything).collect();
        (!panels.is_empty()).then_some(Self { panels, primary })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DisplayAnchor {
    name: String,
    screen: Rect,
}

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

    #[cfg(test)]
    pub(crate) fn unanchored(frame: Rect) -> Self {
        Self::new(frame, None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Whereabouts {
    Orphaned,
    OnScreen(Option<DisplayAnchor>),
}

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
    fn from_declared(
        min_width: f64,
        min_height: f64,
        default_width: f64,
        default_height: f64,
    ) -> Self {
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

fn handle_is_reachable(rect: &Rect, displays: &Displays, policy: &Policy) -> bool {
    let handle = Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: policy.grab_height.min(rect.height),
    };
    let needed_width = policy.grab_width.min(handle.width);
    displays.panels.iter().any(|panel| {
        handle.overlap_width(&panel.work_area) >= needed_width
            && handle.overlap_height(&panel.work_area) >= handle.height
    })
}

fn a_screen_can_hold(rect: &Rect, displays: &Displays) -> bool {
    displays
        .panels
        .iter()
        .any(|panel| panel.frame.width >= rect.width && panel.frame.height >= rect.height)
}

fn home_work_area(rect: &Rect, displays: &Displays, policy: &Policy) -> Option<Rect> {
    displays
        .panels
        .iter()
        .map(|panel| panel.work_area)
        .max_by_key(|work_area| rect.overlap_area(work_area))
        .filter(|work_area| rect.overlap_area(work_area) >= policy.grab_width * policy.grab_height)
}

fn nudge_axis(pos: i64, len: i64, origin: i64, span: i64) -> i64 {
    pos.clamp(origin, origin + (span - len).max(0))
}

pub(crate) fn sanitize(restored: Rect, displays: &Displays, policy: &Policy) -> Option<Rect> {
    let mut fixed = restored;

    if fixed.width < policy.min_width || fixed.height < policy.min_height {
        fixed.width = policy.default_width;
        fixed.height = policy.default_height;
    }

    if !a_screen_can_hold(&fixed, displays) {
        let home = home_work_area(&fixed, displays, policy).unwrap_or(displays.primary);
        fixed.width = fixed.width.min(home.width).max(policy.min_width);
        fixed.height = fixed.height.min(home.height).max(policy.min_height);
        fixed.x = nudge_axis(fixed.x, fixed.width, home.x, home.width);
        fixed.y = nudge_axis(fixed.y, fixed.height, home.y, home.height);
    }

    if !handle_is_reachable(&fixed, displays, policy) {
        match home_work_area(&fixed, displays, policy) {
            Some(work_area) => {
                fixed.x = nudge_axis(fixed.x, fixed.width, work_area.x, work_area.width);
                fixed.y = nudge_axis(fixed.y, fixed.height, work_area.y, work_area.height);
            }
            None => {
                let primary = displays.primary;
                fixed.x = primary.x + (primary.width - fixed.width).max(0) / 2;
                fixed.y = primary.y + (primary.height - fixed.height).max(0) / 2;
            }
        }
    }

    (fixed != restored).then_some(fixed)
}

pub(crate) fn fit_to_displays(live: Rect, displays: &Displays, policy: &Policy) -> Option<Rect> {
    if a_screen_can_hold(&live, displays) {
        return None;
    }
    sanitize(live, displays, policy)
}

const FRESH_SHARE_NUMERATOR: i64 = 4;
const FRESH_SHARE_DENOMINATOR: i64 = 5;

const FRESH_MAX_WIDTH_POINTS: i64 = 1680;
const FRESH_MAX_HEIGHT_POINTS: i64 = 1050;

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

static RESCUED: Mutex<BTreeMap<String, Rect>> = Mutex::new(BTreeMap::new());

fn note_rescued(label: &str, frame: Rect) {
    RESCUED.lock().unwrap().insert(label.to_string(), frame);
}

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

pub(crate) fn forget_rescue(label: &str) {
    RESCUED.lock().unwrap().remove(label);
}

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

pub(crate) fn declared_min_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    let config = policy_config(
        &app.config().app.windows,
        crate::app_window::MAIN_WINDOW_LABEL,
    )?;
    Some((config.min_width?, config.min_height?))
}

fn policy_and_displays(app: &tauri::AppHandle, label: &str) -> Option<(Policy, Displays)> {
    let Some(config) = policy_config(&app.config().app.windows, label).cloned() else {
        eprintln!("[Tauri] No window config to judge `{label}` against: skipping the clamp");
        return None;
    };
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

pub(crate) fn cascade_source(window: &tauri::Window) -> Option<Rect> {
    if window.is_fullscreen().unwrap_or(false) {
        return None;
    }
    live_frame(window)
}

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

pub(crate) fn whereabouts_now(app: &tauri::AppHandle, label: &str, frame: Rect) -> Whereabouts {
    match policy_and_displays(app, label) {
        Some((policy, displays)) => whereabouts(frame, &displays, &policy),
        None => Whereabouts::OnScreen(None),
    }
}

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
            note_rescued(label, fixed);
            fixed
        }
        None => frame,
    }
}

pub(crate) fn clamp_restored_geometry(app: &tauri::AppHandle, label: &str) {
    clamp_geometry(app, label, "Restored window geometry", sanitize);
}

pub(crate) fn clamp_live_geometry(app: &tauri::AppHandle, label: &str) {
    clamp_geometry(app, label, "Live window geometry", fit_to_displays);
}

fn clamp_geometry(
    app: &tauri::AppHandle,
    label: &str,
    what: &str,
    decide: fn(Rect, &Displays, &Policy) -> Option<Rect>,
) {
    let Some(window) = app.get_window(label) else {
        return;
    };
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
    note_rescued(label, fixed);
    crate::app_window::place_window(&window, fixed, &format!("`{label}` back on screen"));
}

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

#[cfg(target_os = "macos")]
pub(crate) fn primary_screen_height(app: &tauri::AppHandle) -> Option<i64> {
    let primary = app.primary_monitor().ok().flatten()?;
    let height = panel_points(&primary).frame.height;
    (height > 0).then_some(height)
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn appkit_top_left(frame: Rect, primary_screen_height: i64) -> (f64, f64) {
    (frame.x as f64, (primary_screen_height - frame.y) as f64)
}

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

    fn window_config(label: &str) -> tauri::utils::config::WindowConfig {
        tauri::utils::config::WindowConfig {
            label: label.to_string(),
            ..Default::default()
        }
    }

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

    #[test]
    fn no_declared_window_leaves_nothing_to_clamp_against() {
        assert!(policy_config(&[], "main").is_none());
    }

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

    fn desk(panels: Vec<Panel>, primary: Rect) -> Displays {
        Displays::new(panels, primary).expect("a fixture desk must hold a window")
    }

    fn plain_panel(rect: Rect) -> Panel {
        Panel {
            name: None,
            work_area: rect,
            frame: rect,
        }
    }

    fn one_panel() -> Displays {
        let panel = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        desk(vec![panel.clone()], panel.work_area)
    }

    fn one_panel_with_a_dock() -> Displays {
        let mut panel = panel_under_a_menu_bar(0, 0, 1728, 1117, 37);
        panel.work_area.height -= 80;
        desk(vec![panel.clone()], panel.work_area)
    }

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

    fn the_laptop_alone() -> Displays {
        let laptop = two_one_x_panels().panels[0].clone();
        desk(vec![laptop.clone()], laptop.work_area)
    }

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

    fn policy() -> Policy {
        Policy::from_declared(480.0, 400.0, 1024.0, 768.0)
    }

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

    #[test]
    fn a_built_in_frame_seats_on_the_built_in() {
        let frame = Rect {
            x: 1763,
            y: 1473,
            width: 1728,
            height: 1084,
        };
        let (x, top) = appkit_top_left(frame, 1440);
        assert_eq!((x, top), (1763.0, -33.0));
        assert_eq!(
            top - frame.height as f64,
            -1117.0,
            "bottom on the built-in's"
        );
    }

    #[test]
    fn a_primary_frame_seats_under_the_primary_top_edge() {
        let frame = Rect {
            x: 0,
            y: 30,
            width: 1280,
            height: 1410,
        };
        assert_eq!(appkit_top_left(frame, 1440), (0.0, 1410.0));
    }

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
        assert!(is_wearing_a_rescue("rescue-test-held", a_rescued_frame()));
        forget_rescue("rescue-test-held");
    }

    #[test]
    fn a_frame_the_user_changed_ends_the_rescue_for_good() {
        note_rescued("rescue-test-moved", a_rescued_frame());
        let moved = Rect {
            x: 200,
            ..a_rescued_frame()
        };
        assert!(!is_wearing_a_rescue("rescue-test-moved", moved));
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

    #[test]
    fn a_desk_with_no_panels_is_not_a_desk() {
        let primary = one_panel().primary;
        assert!(Displays::new(Vec::new(), primary).is_none());
    }

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

    fn reported_primary() -> Rect {
        Rect {
            x: 0,
            y: 33,
            width: 1728,
            height: 1084,
        }
    }

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
        let degenerate = Rect {
            x: 500,
            y: 400,
            width: 1,
            height: 1,
        };
        let fixed = sanitize(degenerate, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.width, fixed.height), (1024, 768));
        assert_eq!((fixed.x, fixed.y), (500, 400));
    }

    #[test]
    fn the_reported_bug_recentres_on_the_primary() {
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
        let hanging = Rect {
            x: 1628,
            y: 250,
            width: 1024,
            height: 768,
        };
        let fixed = sanitize(hanging, &one_panel(), &policy()).expect("must be corrected");
        assert_eq!((fixed.x, fixed.y), (704, 250));
        assert_eq!((fixed.width, fixed.height), (1024, 768));
    }

    #[test]
    fn a_sliver_on_the_edge_counts_as_gone_and_recentres() {
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

    #[test]
    fn a_frame_another_attached_display_can_hold_keeps_its_size() {
        let wide = Rect {
            x: 1850,
            y: 60,
            width: 2000,
            height: 1200,
        };
        assert_eq!(sanitize(wide, &two_panels(), &policy()), None);
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
        assert_eq!((fixed.width, fixed.height), (1728, 1080));
    }

    #[test]
    fn a_window_reaching_under_the_dock_keeps_its_size() {
        let displays = one_panel_with_a_dock();
        let over_the_dock = Rect {
            x: 0,
            y: 37,
            width: 1728,
            height: 1080,
        };
        assert_eq!(sanitize(over_the_dock, &displays, &policy()), None);
    }

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
            Rect {
                x: 600,
                y: 400,
                width: 2048,
                height: 1536,
            },
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
        assert!(number("width") >= number("minWidth"));
        assert!(number("height") >= number("minHeight"));
    }

    const EXTERNAL: &str = "Monitor #41003";
    const BUILT_IN: &str = "Monitor #41216";

    fn named(name: &str, panel: Panel) -> Panel {
        Panel {
            name: Some(name.to_string()),
            ..panel
        }
    }

    fn docked_desk() -> Displays {
        let external = named(EXTERNAL, panel_under_a_menu_bar(0, 0, 5120, 1440, 30));
        let built_in = named(BUILT_IN, panel_under_a_menu_bar(1763, 1440, 1728, 1117, 32));
        let primary = external.work_area;
        desk(vec![external, built_in], primary)
    }

    fn undocked_desk() -> Displays {
        let built_in = named(BUILT_IN, panel_under_a_menu_bar(0, 0, 1728, 1117, 32));
        let primary = built_in.work_area;
        desk(vec![built_in], primary)
    }

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

    #[test]
    fn a_frame_on_a_display_that_has_not_moved_comes_back_unchanged() {
        let frame = at(643, 132, 1728, 1084);
        assert_eq!(
            resolve(&captured(frame, &docked_desk()), &docked_desk()),
            frame
        );
    }

    #[test]
    fn a_frame_whose_display_is_away_comes_back_raw() {
        let frame = at(643, 132, 1728, 1084);
        let remembered = captured(frame, &docked_desk());
        assert_eq!(resolve(&remembered, &undocked_desk()), frame);
    }

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

    #[test]
    fn a_frame_on_an_unnamed_display_is_on_screen_and_unanchored() {
        assert_eq!(
            whereabouts(at(100, 100, 1200, 800), &one_panel(), &policy()),
            Whereabouts::OnScreen(None)
        );
    }

    #[test]
    fn an_anchored_frame_is_readable_by_an_older_build() {
        let remembered = captured(at(1763, 1473, 1728, 1084), &docked_desk());
        let json = serde_json::to_string(&remembered).expect("serialize");
        let as_older_build: Rect = serde_json::from_str(&json).expect("an older build reads it");
        assert_eq!(as_older_build, at(1763, 1473, 1728, 1084));
        let back: RememberedFrame = serde_json::from_str(&json).expect("this build reads it");
        assert_eq!(back, remembered);
    }

    #[test]
    fn an_unanchored_frame_writes_the_old_shape() {
        let json = serde_json::to_value(RememberedFrame::unanchored(at(1, 2, 1200, 800)))
            .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({"x": 1, "y": 2, "width": 1200, "height": 800})
        );
    }

    #[test]
    fn a_new_window_cascades_from_its_source_at_the_same_size() {
        assert_eq!(
            cascade(at(100, 137, 1400, 900), &one_panel(), &policy()),
            at(128, 165, 1400, 900)
        );
    }

    #[test]
    fn a_cascade_off_the_bottom_shrinks_to_fit() {
        let work = one_panel().primary;
        let source = at(200, work.bottom() - 900, 1400, 900);
        assert_eq!(
            cascade(source, &one_panel(), &policy()),
            at(228, 245, 1400, 872)
        );
    }

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

    #[test]
    fn a_fresh_window_is_at_least_the_declared_default_that_fits() {
        let small = plain_panel(at(0, 0, 1152, 720));
        let fresh = fresh(&desk(vec![small.clone()], small.work_area), &policy());
        assert_eq!((fresh.width, fresh.height), (1024, 720));
    }
}
