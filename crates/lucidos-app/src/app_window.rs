use std::collections::BTreeMap;
use std::sync::Mutex;

use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::{
    activation, desktop, notifications, traffic_lights, window_persist, window_restore,
    window_screen, window_target,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Placement {
    MoveTo { x: f64, y: f64 },
    ResizeTo { width: f64, height: f64 },
}

impl Placement {
    fn verb(&self) -> &'static str {
        match self {
            Placement::MoveTo { .. } => "move",
            Placement::ResizeTo { .. } => "resize",
        }
    }
}

fn placement_steps(frame: window_restore::Rect) -> [Placement; 2] {
    [
        Placement::MoveTo {
            x: frame.x as f64,
            y: frame.y as f64,
        },
        Placement::ResizeTo {
            width: frame.width as f64,
            height: frame.height as f64,
        },
    ]
}

pub(crate) fn place_window(window: &tauri::Window, frame: window_restore::Rect, what: &str) {
    for step in placement_steps(frame) {
        let applied = match step {
            Placement::MoveTo { x, y } => window.set_position(tauri::LogicalPosition::new(x, y)),
            Placement::ResizeTo { width, height } => {
                window.set_size(tauri::LogicalSize::new(width, height))
            }
        };
        if let Err(e) = applied {
            eprintln!("[Tauri] Failed to {} {what}: {e}", step.verb());
        }
    }
}

pub(crate) fn window_content_size(
    window: &tauri::Window,
) -> tauri::Result<tauri::PhysicalSize<u32>> {
    window.outer_size()
}

const REFIT_TOLERANCE_PX: i64 = 1;

fn webview_fills_window(
    window: tauri::PhysicalSize<u32>,
    webview: tauri::PhysicalSize<u32>,
    offset: tauri::PhysicalPosition<i32>,
) -> bool {
    let near = |a: i64, b: i64| (a - b).abs() <= REFIT_TOLERANCE_PX;
    near(i64::from(window.width), i64::from(webview.width))
        && near(i64::from(window.height), i64::from(webview.height))
        && near(i64::from(offset.x), 0)
        && near(i64::from(offset.y), 0)
}

pub(crate) fn refit_webview(app: &tauri::AppHandle, label: &str) {
    let Some(webview) = app.get_webview(label) else {
        return;
    };
    let window = webview.window();
    let Ok(size) = window_content_size(&window) else {
        eprintln!("[Tauri] Could not read {label} to refit its webview");
        return;
    };
    if let (Ok(page), Ok(at)) = (webview.size(), webview.position()) {
        if webview_fills_window(size, page, at) {
            return;
        }
    }
    let Ok(scale) = window.scale_factor() else {
        eprintln!("[Tauri] Could not read the scale factor of {label} to refit its webview");
        return;
    };
    if !scale.is_finite() || scale <= 0.0 {
        eprintln!("[Tauri] {label} reports a scale factor of {scale}: skipping the refit");
        return;
    }
    let bounds = tauri::Rect {
        position: tauri::LogicalPosition::new(0.0, 0.0).into(),
        size: tauri::LogicalSize::new(size.width as f64 / scale, size.height as f64 / scale).into(),
    };
    if let Err(e) = webview.set_bounds(bounds) {
        eprintln!("[Tauri] Failed to refit the webview of {label}: {e}");
    }
}

const DEFAULT_WIDTH_POINTS: f64 = 1024.0;
const DEFAULT_HEIGHT_POINTS: f64 = 768.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct BuildGeometry {
    width: f64,
    height: f64,
    position: Option<(f64, f64)>,
}

fn build_geometry(frame: Option<window_restore::Rect>) -> BuildGeometry {
    match frame {
        Some(frame) => BuildGeometry {
            width: frame.width as f64,
            height: frame.height as f64,
            position: Some((frame.x as f64, frame.y as f64)),
        },
        None => BuildGeometry {
            width: DEFAULT_WIDTH_POINTS,
            height: DEFAULT_HEIGHT_POINTS,
            position: None,
        },
    }
}

static WEBVIEW_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(crate) fn next_webview_label_counter() -> u32 {
    WEBVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

pub(crate) fn window_order_key(label: &str) -> (bool, &str) {
    (label != MAIN_WINDOW_LABEL, label)
}

const APP_WINDOW_PREFIX: &str = "window-";

pub(crate) fn is_app_window(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL || label.starts_with(APP_WINDOW_PREFIX)
}

pub(crate) fn should_retain_on_close(label: &str, development: bool, tearing_down: bool) -> bool {
    !development && !tearing_down && is_app_window(label)
}

fn fresh_app_window_label() -> String {
    format!("{APP_WINDOW_PREFIX}{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn is_restorable_label(label: &str) -> bool {
    if label == MAIN_WINDOW_LABEL {
        return true;
    }
    label.strip_prefix(APP_WINDOW_PREFIX).is_some_and(|id| {
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn label_for_new_window(recorded: Option<&str>, in_use: impl Fn(&str) -> bool) -> String {
    recorded
        .filter(|label| *label != MAIN_WINDOW_LABEL)
        .filter(|label| is_restorable_label(label) && !in_use(label))
        .map_or_else(fresh_app_window_label, str::to_string)
}

#[tauri::command]
pub(crate) fn start_window_drag(window: tauri::Window) -> Result<(), String> {
    window.start_dragging().map_err(|e| format!("{e}"))
}

#[tauri::command]
pub(crate) fn toggle_window_maximize(window: tauri::Window) -> Result<(), String> {
    if window.is_maximized().map_err(|e| format!("{e}"))? {
        window.unmaximize().map_err(|e| format!("{e}"))
    } else {
        window.maximize().map_err(|e| format!("{e}"))
    }
}

#[tauri::command]
pub(crate) fn set_window_fullscreen(
    window: tauri::Window,
    fullscreen: bool,
) -> Result<bool, String> {
    let was_fullscreen = window.is_fullscreen().map_err(|e| format!("{e}"))?;
    window
        .set_fullscreen(fullscreen)
        .map_err(|e| format!("{e}"))?;
    Ok(was_fullscreen)
}

const WINDOW_FULLSCREEN_CHANGED_EVENT: &str = "window-fullscreen-changed";

static FULLSCREEN_REPORTED: Mutex<BTreeMap<String, bool>> = Mutex::new(BTreeMap::new());

fn fullscreen_changed(reported: &mut BTreeMap<String, bool>, label: &str, now: bool) -> bool {
    reported.insert(label.to_string(), now).unwrap_or(false) != now
}

pub(crate) fn report_window_fullscreen(window: &tauri::Window) {
    let Ok(now) = window.is_fullscreen() else {
        return;
    };
    let changed = fullscreen_changed(
        &mut FULLSCREEN_REPORTED.lock().unwrap(),
        window.label(),
        now,
    );
    if changed {
        let _ = window.emit_to(window.label(), WINDOW_FULLSCREEN_CHANGED_EVENT, now);
    }
}

pub(crate) fn forget_window_fullscreen(label: &str) {
    FULLSCREEN_REPORTED.lock().unwrap().remove(label);
}

static FOCUS_ORDER: Mutex<(u64, BTreeMap<String, u64>)> = Mutex::new((0, BTreeMap::new()));

pub(crate) fn record_window_focus(label: &str) {
    let mut order = FOCUS_ORDER.lock().unwrap();
    order.0 += 1;
    let seq = order.0;
    order.1.insert(label.to_string(), seq);
}

pub(crate) fn forget_window_focus(label: &str) {
    FOCUS_ORDER.lock().unwrap().1.remove(label);
}

fn last_focused(label: &str) -> Option<u64> {
    FOCUS_ORDER.lock().unwrap().1.get(label).copied()
}

#[tauri::command]
pub(crate) fn set_window_title(window: tauri::Window, title: String) -> Result<(), String> {
    traffic_lights::retitle(&window, title)
}

pub(crate) fn open_new_window(app: &tauri::AppHandle) -> Result<(), String> {
    let source =
        source_webview(app).and_then(|webview| window_restore::cascade_source(&webview.window()));
    open_app_window(app, new_window_url(app), Birthplace::Beside(source), None)
}

enum Birthplace {
    Remembered(window_restore::RememberedFrame),
    Beside(Option<window_restore::Rect>),
}

impl Birthplace {
    fn remembered_or_beside(
        remembered: Option<window_restore::RememberedFrame>,
        source: Option<window_restore::Rect>,
    ) -> Self {
        match remembered {
            Some(frame) => Birthplace::Remembered(frame),
            None => Birthplace::Beside(source),
        }
    }
}

fn open_app_window(
    app: &tauri::AppHandle,
    url: WebviewUrl,
    birthplace: Birthplace,
    recorded: Option<&str>,
) -> Result<(), String> {
    let label = label_for_new_window(recorded, |label| app.get_window(label).is_some());
    let frame = match birthplace {
        Birthplace::Remembered(remembered) => {
            Some(window_restore::sanitized_frame(app, &label, &remembered))
        }
        Birthplace::Beside(source) => window_restore::new_window_frame(app, &label, source),
    };

    let geometry = build_geometry(frame);
    let builder = WebviewWindowBuilder::new(app, &label, url)
        .title("Lucidos")
        .inner_size(geometry.width, geometry.height)
        .disable_drag_drop_handler();
    let builder = match window_restore::declared_min_size(app) {
        Some((width, height)) => builder.min_inner_size(width, height),
        None => builder,
    };
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    let builder = match geometry.position {
        Some((x, y)) => builder.position(x, y).visible(false),
        None => builder,
    };
    let placed = geometry.position.is_some();
    let window = builder.build().map_err(|e| format!("{e}"))?;
    if let (true, Some(frame)) = (placed, frame) {
        seat_at_birth(app, &window, frame);
    }
    if let Some(color) = crate::pre_paint_title_bar_color(app) {
        crate::paint_title_bars(app, color);
    }
    traffic_lights::place_all(app);
    let mut on_screen = !placed;
    if placed {
        match window.show() {
            Ok(()) => on_screen = true,
            Err(e) => eprintln!("[Tauri] Failed to show the restored window {label}: {e}"),
        }
    }
    if on_screen {
        note_window_shown(app, &label, true);
    }
    window_persist::note_presented();
    Ok(())
}

#[cfg(target_os = "macos")]
fn seat_at_birth(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    frame: window_restore::Rect,
) {
    let label = window.label();
    if objc2::MainThreadMarker::new().is_none() {
        eprintln!(
            "[Tauri] {label} was built off the main thread: leaving its seat to the queued move"
        );
        return;
    }
    let Some(primary_height) = window_restore::primary_screen_height(app) else {
        eprintln!(
            "[Tauri] No primary display to seat {label} against: leaving it to the queued move"
        );
        return;
    };
    let ptr = match window.ns_window() {
        Ok(ptr) if !ptr.is_null() => ptr,
        _ => {
            eprintln!("[Tauri] No NSWindow to seat {label}: leaving it to the queued move");
            return;
        }
    };
    // SAFETY: `ns_window` hands back this window's `NSWindow`, alive for the
    // rest of this call, and the marker above proves this is the main thread.
    let ns_window: &objc2_app_kit::NSWindow = unsafe { &*ptr.cast() };
    let (x, y) = window_restore::appkit_top_left(frame, primary_height);
    let mut top_left = ns_window.frame().origin;
    top_left.x = x;
    top_left.y = y;
    ns_window.setFrameTopLeftPoint(top_left);
}

#[cfg(not(target_os = "macos"))]
fn seat_at_birth(_: &tauri::AppHandle, _: &tauri::WebviewWindow, _: window_restore::Rect) {}

pub(crate) fn source_webview(app: &tauri::AppHandle) -> Option<tauri::Webview> {
    let focused = app
        .webviews()
        .into_iter()
        .filter(|(label, _)| is_app_window(label))
        .find(|(_, webview)| webview.window().is_focused().unwrap_or(false))
        .map(|(_, webview)| webview);
    focused.or_else(|| app.get_webview(MAIN_WINDOW_LABEL))
}

fn new_window_url(app: &tauri::AppHandle) -> WebviewUrl {
    if let Some(url) = source_webview(app).and_then(|w| w.url().ok()) {
        if url.scheme() == "http" || url.scheme() == "https" {
            return WebviewUrl::External(url);
        }
    }
    if !tauri::is_dev() {
        if let Ok(url) = desktop::gateway_url(desktop::engine_port()).parse::<tauri::Url>() {
            return WebviewUrl::External(url);
        }
    }
    WebviewUrl::App("index.html".into())
}

fn live_app_windows(app: &tauri::AppHandle) -> Vec<desktop::LiveWindow> {
    app.webviews()
        .into_iter()
        .filter(|(label, _)| is_app_window(label))
        .map(|(label, webview)| {
            let url = webview.url().map(|u| u.to_string()).unwrap_or_else(|e| {
                eprintln!("[Tauri] Could not read the URL of window {label}: {e}");
                String::new()
            });
            desktop::LiveWindow {
                url,
                visible: webview.window().is_visible().unwrap_or(false),
                minimized: webview.window().is_minimized().unwrap_or(false),
                last_focused: last_focused(&label),
                label,
            }
        })
        .collect()
}

fn parse_window_url(url: &str) -> Result<tauri::Url, String> {
    url.parse::<tauri::Url>()
        .map_err(|e| format!("could not open {url}: {e}"))
}

#[tauri::command]
pub(crate) fn show_workspace_window(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    workspace: String,
    landing: Option<String>,
) -> Result<(), String> {
    let landing = match landing.as_deref() {
        None => None,
        Some(name) => Some(
            window_target::WorkspaceLanding::parse(name)
                .ok_or_else(|| format!("{name:?} is not a landing"))?,
        ),
    };
    let caller = webview.url().ok().map(|u| u.to_string());
    let fallback = desktop::gateway_url(desktop::engine_port());
    let origin = window_target::target_origin(caller.as_deref(), &fallback);
    let windows = live_app_windows(&app);
    let window = webview.window();
    let target = window_target::choose_workspace_target(
        &windows,
        window.label(),
        &workspace,
        origin,
        landing,
    )
    .ok_or_else(|| format!("{workspace:?} is not a workspace"))?;

    match target {
        window_target::WorkspaceTarget::Focus(label) => {
            front_window(&app, &label);
            Ok(())
        }
        window_target::WorkspaceTarget::Navigate { label, url } => {
            let parsed = parse_window_url(&url)?;
            let target = app
                .get_webview(&label)
                .ok_or_else(|| format!("could not point {label} at {url}: no such window"))?;
            target
                .navigate(parsed)
                .map_err(|e| format!("could not point {label} at {url}: {e}"))?;
            front_window(&app, &label);
            Ok(())
        }
        window_target::WorkspaceTarget::NewWindow { url } => {
            let birthplace = Birthplace::remembered_or_beside(
                window_persist::remembered_frame(&url),
                window_restore::cascade_source(&window),
            );
            open_app_window(
                &app,
                WebviewUrl::External(parse_window_url(&url)?),
                birthplace,
                None,
            )
        }
    }
}

pub(crate) fn settle_main_geometry(
    app: &tauri::AppHandle,
    frame: Option<window_restore::RememberedFrame>,
) {
    match frame {
        Some(remembered) => {
            let frame = window_restore::sanitized_frame(app, MAIN_WINDOW_LABEL, &remembered);
            window_persist::size_main_window_for_its_workspace(app, frame);
        }
        None => window_restore::clamp_restored_geometry(app, MAIN_WINDOW_LABEL),
    }
    MAIN_GEOMETRY_SETTLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

static MAIN_GEOMETRY_SETTLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn main_frame_owed(
    navigated: Option<Option<window_restore::RememberedFrame>>,
    settled: bool,
    remembered: Option<window_restore::RememberedFrame>,
) -> Option<window_restore::RememberedFrame> {
    match navigated {
        Some(frame) => frame,
        None if settled => None,
        None => remembered,
    }
}

fn main_frame_owed_now(
    live: &[desktop::LiveWindow],
    plan: &desktop::ReopenPlan,
) -> Option<window_restore::RememberedFrame> {
    let settled = MAIN_GEOMETRY_SETTLED.load(std::sync::atomic::Ordering::SeqCst);
    main_frame_owed(
        plan.navigate_main
            .as_ref()
            .map(|planned| planned.frame.clone()),
        settled,
        (!settled)
            .then(|| {
                live.iter()
                    .find(|window| window.label == MAIN_WINDOW_LABEL)
                    .and_then(|window| window_persist::remembered_frame(&window.url))
            })
            .flatten(),
    )
}

pub(crate) fn restore_extra_windows(app: &tauri::AppHandle, windows: &[desktop::PlannedWindow]) {
    for window in windows {
        let url = &window.url;
        let Ok(parsed) = url.parse::<tauri::Url>() else {
            eprintln!("[Tauri] Cannot restore a window on an unparseable URL: {url}");
            continue;
        };
        let birthplace = Birthplace::remembered_or_beside(window.frame.clone(), None);
        let recorded = window.label.as_deref();
        if let Err(e) = open_app_window(app, WebviewUrl::External(parsed), birthplace, recorded) {
            eprintln!("[Tauri] Failed to restore a window on {url}: {e}");
        }
    }
}

pub(crate) fn hide_all_windows(app: &tauri::AppHandle) {
    for (label, window) in app.windows() {
        if window.hide().is_ok() {
            note_window_shown(app, &label, false);
        }
    }
}

#[tauri::command]
pub(crate) fn focus_calling_window(app: tauri::AppHandle, window: tauri::Window) {
    activation::set_menu_bar_only(&app, false);
    let _ = window.unminimize();
    if window.show().is_err() {
        return;
    }
    let _ = window.set_focus();
    activation::activate_app_frontmost();
    note_window_shown(&app, window.label(), true);
    emit_window_active(&app, window.label(), true);
    window_persist::note_presented();
}

#[tauri::command]
pub(crate) fn get_native_window_active(window: tauri::Window) -> bool {
    let focused = window.is_focused().unwrap_or(false);
    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    focused && visible && !minimized
}

#[tauri::command]
pub(crate) fn get_native_window_shown(window: tauri::Window) -> bool {
    window_screen::shown_by_client(window.label())
}

pub(crate) fn note_window_shown(app: &tauri::AppHandle, label: &str, shown: bool) {
    if shown {
        window_screen::note_shown(label);
    } else {
        window_screen::note_hidden(label);
    }
    let _ = app.emit_to(label, "native-window-shown", shown);
}

pub(crate) fn emit_window_active(app: &tauri::AppHandle, label: &str, active: bool) {
    let _ = app.emit_to(label, "native-window-active", active);
}

pub(crate) fn visible_app_windows(app: &tauri::AppHandle, excluding: Option<&str>) -> usize {
    app.windows()
        .iter()
        .filter(|(label, w)| {
            is_app_window(label.as_str())
                && Some(label.as_str()) != excluding
                && w.is_visible().unwrap_or(false)
        })
        .count()
}

pub(crate) fn enter_menu_bar_only_if_no_windows(app: &tauri::AppHandle, excluding: Option<&str>) {
    if activation::should_be_menu_bar_only(visible_app_windows(app, excluding)) {
        activation::set_menu_bar_only(app, true);
    }
}

pub(crate) fn hide_app_window(window: &tauri::Window) {
    let app = window.app_handle();
    if let Err(error) = window.hide() {
        eprintln!("[Window] Could not hide {}: {error}", window.label());
        return;
    }
    note_window_shown(app, window.label(), false);
    emit_window_active(app, window.label(), false);
    enter_menu_bar_only_if_no_windows(app, None);
}

pub(crate) fn close_all_to_tray(app: &tauri::AppHandle) {
    if tauri::is_dev() {
        window_persist::persist_windows(app);
        window_persist::begin_teardown();
        for (label, window) in app.windows() {
            if is_app_window(&label) {
                let _ = window.close();
            }
        }
        return;
    }
    window_persist::persist_windows(app);
    for (label, window) in app.windows() {
        if is_app_window(&label) {
            hide_app_window(&window);
        }
    }
    enter_menu_bar_only_if_no_windows(app, None);
}

pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if app.get_window(MAIN_WINDOW_LABEL).is_some() {
        front_window(app, MAIN_WINDOW_LABEL);
        return;
    }
    activation::set_menu_bar_only(app, false);
    if let Err(e) = open_new_window(app) {
        eprintln!("[Tauri] Failed to open window: {e}");
    }
}

pub(crate) fn reopen_client(app: &tauri::AppHandle) {
    activation::set_menu_bar_only(app, false);
    let live = live_app_windows(app);
    let origin = notifications::gateway_origin(&live)
        .map(str::to_string)
        .unwrap_or_else(|| desktop::gateway_url(desktop::engine_port()));
    let plan = desktop::reopen_plan(&live, &window_persist::readable_window_session(), &origin);

    if let Some(planned) = &plan.navigate_main {
        desktop::navigate_main_window(app, &planned.url);
    }
    settle_main_geometry(app, main_frame_owed_now(&live, &plan));
    for label in &plan.show {
        if let Some(window) = app.get_window(label) {
            let _ = window.unminimize();
            match window.show() {
                Ok(()) => note_window_shown(app, label, true),
                Err(e) => eprintln!("[Tauri] Failed to show the parked window {label}: {e}"),
            }
        }
    }
    restore_extra_windows(app, &plan.build);
    match &plan.front {
        Some(label) => front_window(app, label),
        None if !plan.build.is_empty() => activation::activate_app_frontmost(),
        None => show_main_window(app),
    }
}

fn front_window(app: &tauri::AppHandle, label: &str) {
    activation::set_menu_bar_only(app, false);
    if !window_screen::shown_by_client(label) {
        window_restore::clamp_restored_geometry(app, label);
    }
    if let Some(window) = app.get_window(label) {
        let _ = window.unminimize();
        if window.show().is_err() {
            return;
        }
        let _ = window.set_focus();
        activation::activate_app_frontmost();
        note_window_shown(app, label, true);
        emit_window_active(app, label, true);
        window_persist::note_presented();
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn route_native_tap(app: &tauri::AppHandle, owner: Option<&str>) -> Option<String> {
    let windows = live_app_windows(app);

    let origin = notifications::gateway_origin(&windows)
        .map(str::to_string)
        .unwrap_or_else(|| desktop::gateway_url(desktop::engine_port()));

    match notifications::choose_tap_target(&windows, owner, &origin) {
        notifications::TapTarget::Focus(label) => {
            front_window(app, &label);
            Some(label)
        }
        notifications::TapTarget::Navigate { label, url } => {
            match (app.get_webview(&label), url.parse::<tauri::Url>()) {
                (Some(window), Ok(parsed)) => {
                    if let Err(e) = window.navigate(parsed) {
                        eprintln!("[Tauri] Failed to point {label} at {url}: {e}");
                    }
                }
                _ => eprintln!("[Tauri] Cannot point {label} at {url}: no such window / bad URL"),
            }
            front_window(app, &label);
            None
        }
        notifications::TapTarget::NewWindow { url } => {
            let birthplace =
                Birthplace::remembered_or_beside(window_persist::remembered_frame(&url), None);
            match url.parse::<tauri::Url>() {
                Ok(parsed) => {
                    activation::set_menu_bar_only(app, false);
                    if let Err(e) =
                        open_app_window(app, WebviewUrl::External(parsed), birthplace, None)
                    {
                        eprintln!("[Tauri] Failed to open a window for {url}: {e}");
                    }
                    activation::activate_app_frontmost();
                }
                Err(e) => eprintln!("[Tauri] Bad tap target URL {url}: {e}"),
            }
            None
        }
        notifications::TapTarget::LaunchInto { url } => {
            desktop::set_launch_target(url);
            show_main_window(app);
            None
        }
        notifications::TapTarget::MainWindow => {
            show_main_window(app);
            app.get_window(MAIN_WINDOW_LABEL)
                .map(|_| MAIN_WINDOW_LABEL.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_packaged_closes_retain_every_app_window() {
        for label in ["main", "window-0", "window-secondary"] {
            assert!(should_retain_on_close(label, false, false));
            assert!(!should_retain_on_close(label, true, false));
            assert!(!should_retain_on_close(label, false, true));
            assert!(!should_retain_on_close(label, true, true));
        }
        for label in ["url-preview-3", "lucidos-tray", ""] {
            assert!(!should_retain_on_close(label, false, false));
        }
    }

    fn reported_frame() -> window_restore::Rect {
        window_restore::Rect {
            x: 1763,
            y: 1473,
            width: 1829,
            height: 1084,
        }
    }

    #[test]
    fn a_remembered_frame_is_where_the_window_is_born() {
        assert_eq!(
            build_geometry(Some(reported_frame())),
            BuildGeometry {
                width: 1829.0,
                height: 1084.0,
                position: Some((1763.0, 1473.0)),
            }
        );
    }

    #[test]
    fn no_frame_takes_the_declared_default_and_no_position() {
        assert_eq!(
            build_geometry(None),
            BuildGeometry {
                width: 1024.0,
                height: 768.0,
                position: None,
            }
        );
    }

    #[test]
    fn the_default_size_is_the_declared_one() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let main = config["app"]["windows"]
            .as_array()
            .and_then(|windows| windows.first())
            .expect("a main window");
        assert_eq!(main["width"].as_f64(), Some(DEFAULT_WIDTH_POINTS));
        assert_eq!(main["height"].as_f64(), Some(DEFAULT_HEIGHT_POINTS));
    }

    #[test]
    fn a_placement_moves_before_it_resizes() {
        assert_eq!(
            placement_steps(reported_frame()),
            [
                Placement::MoveTo {
                    x: 1763.0,
                    y: 1473.0
                },
                Placement::ResizeTo {
                    width: 1829.0,
                    height: 1084.0
                },
            ]
        );
    }

    #[test]
    fn a_failed_step_says_which_one_it_was() {
        let [moved, resized] = placement_steps(reported_frame());
        assert_eq!(moved.verb(), "move");
        assert_eq!(resized.verb(), "resize");
    }

    fn size(width: u32, height: u32) -> tauri::PhysicalSize<u32> {
        tauri::PhysicalSize::new(width, height)
    }

    fn at(x: i32, y: i32) -> tauri::PhysicalPosition<i32> {
        tauri::PhysicalPosition::new(x, y)
    }

    #[test]
    fn a_page_wider_than_its_window_owes_a_refit() {
        assert!(!webview_fills_window(
            size(2560, 1410),
            size(4297, 1410),
            at(0, 0)
        ));
    }

    #[test]
    fn a_page_that_already_fills_its_window_owes_nothing() {
        assert!(webview_fills_window(
            size(3267, 1410),
            size(3267, 1410),
            at(0, 0)
        ));
    }

    #[test]
    fn a_single_pixel_of_rounding_is_not_a_mismatch() {
        for page in [size(2559, 1410), size(2561, 1410), size(2560, 1409)] {
            assert!(
                webview_fills_window(size(2560, 1410), page, at(0, 0)),
                "{page:?}"
            );
        }
        for page in [size(2558, 1410), size(2560, 1412)] {
            assert!(
                !webview_fills_window(size(2560, 1410), page, at(0, 0)),
                "{page:?}"
            );
        }
    }

    #[test]
    fn a_page_the_right_size_in_the_wrong_corner_owes_a_refit() {
        assert!(!webview_fills_window(
            size(2560, 1410),
            size(2560, 1410),
            at(40, 0)
        ));
        assert!(!webview_fills_window(
            size(2560, 1410),
            size(2560, 1410),
            at(0, -40)
        ));
    }

    fn frame() -> window_restore::RememberedFrame {
        window_restore::RememberedFrame::unanchored(window_restore::Rect {
            x: 573,
            y: 30,
            width: 3267,
            height: 1410,
        })
    }

    #[test]
    fn a_navigated_main_takes_the_frame_it_is_navigating_to() {
        for settled in [false, true] {
            assert_eq!(
                main_frame_owed(Some(Some(frame())), settled, None),
                Some(frame()),
                "settled {settled}"
            );
            assert_eq!(main_frame_owed(Some(None), settled, Some(frame())), None);
        }
    }

    #[test]
    fn an_unsettled_main_takes_the_frame_its_workspace_remembers() {
        assert_eq!(main_frame_owed(None, false, Some(frame())), Some(frame()));
        assert_eq!(main_frame_owed(None, false, None), None);
    }

    #[test]
    fn a_settled_main_is_left_exactly_where_it_is() {
        assert_eq!(main_frame_owed(None, true, Some(frame())), None);
    }

    #[test]
    fn a_fresh_label_is_one_a_restore_accepts_and_never_recurs() {
        let first = fresh_app_window_label();
        assert!(is_restorable_label(&first));
        assert!(is_app_window(&first));
        assert_ne!(first, fresh_app_window_label());
    }

    #[test]
    fn only_a_label_this_build_makes_is_restorable() {
        assert!(is_restorable_label("main"));
        assert!(is_restorable_label(
            "window-0123456789abcdef0123456789abcdef"
        ));
        for forged in [
            "window-1",
            "window-0123456789ABCDEF0123456789ABCDEF",
            "window-0123456789abcdef0123456789abcdef0",
            "window-../../../../0123456789abcdef0123",
            "url-preview-1",
            "Main",
            "",
        ] {
            assert!(!is_restorable_label(forged), "{forged:?}");
        }
    }

    #[test]
    fn a_new_window_reuses_its_recorded_label_only_when_free() {
        let recorded = "window-0123456789abcdef0123456789abcdef";
        assert_eq!(label_for_new_window(Some(recorded), |_| false), recorded);
        assert_ne!(label_for_new_window(Some(recorded), |_| true), recorded);
        assert_ne!(label_for_new_window(Some("main"), |_| false), "main");
        assert!(is_restorable_label(&label_for_new_window(None, |_| false)));
    }

    #[test]
    fn is_app_window_distinguishes_app_windows_from_panel_previews() {
        assert!(is_app_window("main"));
        assert!(is_app_window("window-0"));
        assert!(is_app_window("window-42"));
        assert!(!is_app_window("url-preview-3"));
        assert!(!is_app_window("lucidos-tray"));
        assert!(!is_app_window(""));
    }

    #[test]
    fn fullscreen_is_reported_on_a_change_only() {
        let mut reported = BTreeMap::new();
        assert!(!fullscreen_changed(&mut reported, "main", false));
        assert!(fullscreen_changed(&mut reported, "main", true));
        assert!(!fullscreen_changed(&mut reported, "main", true));
        assert!(fullscreen_changed(&mut reported, "main", false));
    }

    #[test]
    fn each_window_has_its_own_last_report() {
        let mut reported = BTreeMap::new();
        assert!(fullscreen_changed(&mut reported, "main", true));
        assert!(fullscreen_changed(&mut reported, "window-1", true));
        assert!(!fullscreen_changed(&mut reported, "main", true));
    }
}
