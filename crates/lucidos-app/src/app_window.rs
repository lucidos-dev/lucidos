//! The client's top-level app windows: which ones exist, what each shows, and
//! every way one is opened, fronted, parked or reaped.
//!
//! One concept holds the module together. An app window is `main` or a
//! `window-<n>` built by File > New Window. It carries exactly one webview
//! under its own label, and each can sit on its own workspace. A `url-preview-*`
//! child webview is NOT one, which is what [`is_app_window`] exists to say.
//!
//! That distinction decides how a lookup is spelled. A window operation asks
//! the manager for a `tauri::Window`, a page operation for a `tauri::Webview`.
//! Neither ever asks for a `WebviewWindow`. ADR 0140 has the reason: a window
//! hosting a preview stops answering that third flavour, in silence.
//!
//! The interface out of here is wide, because `run()` calls in from many
//! places: menu items, tray items, window events and notification taps. The
//! concept underneath is single, so the module is cohesive rather than deep.

use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::{
    activation, desktop, notifications, traffic_lights, window_persist, window_restore,
    window_screen, window_target,
};

/// One step of a placement: where the window goes, or how big it is.
///
/// A named pair rather than two straight-line calls, so the ORDER is a value a
/// test can read. Nothing else in the crate can observe it, and getting it
/// wrong halves the page in silence.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Placement {
    MoveTo { x: f64, y: f64 },
    ResizeTo { width: f64, height: f64 },
}

impl Placement {
    /// The verb the log line uses when this step fails.
    fn verb(&self) -> &'static str {
        match self {
            Placement::MoveTo { .. } => "move",
            Placement::ResizeTo { .. } => "resize",
        }
    }
}

/// The steps that put a window at `frame`, in the order they must go out.
///
/// The MOVE goes first. A move can change the window's scale factor, and a
/// resize already queued behind it is then read at the wrong one. tao mints its
/// `Resized` in physical pixels, times the factor at that moment, and the
/// runtime divides by the factor at DRAIN time. Size then move across a scale
/// boundary and the app window's child webview comes out at half the window
/// (ADR 0178).
///
/// Moving first cannot straddle. A resize that itself crosses displays is safe:
/// tao reads the live factor when `windowDidResize:` fires, so the mint and the
/// read agree.
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

/// Put a window at `frame`, in the logical points the record stores.
///
/// The one routine that puts a window anywhere. `main` comes through it before
/// the deferred show, and so does the clamp's own correction. An extra window
/// no longer does: it is BORN at its frame instead (see [`build_geometry`]).
/// Best-effort and logged, since a window at the wrong size beats no window.
///
/// Takes a `tauri::Window`, per ADR 0140: sizing and placing are window
/// operations, and every caller already holds one.
///
/// Logical, so tao applies the numbers as they are. It divides a PHYSICAL one
/// by the scale factor of the display the window is on, which is not the
/// display the frame was captured on (ADR 0173).
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

/// How big a window's content is, in physical pixels. Ask this, never
/// `tauri::Window::inner_size()`.
///
/// **On macOS that getter does not answer about the window.** For a window with
/// no `add_child` webview the runtime returns the first WEBVIEW's NSView frame
/// instead, and deliberately: wry replaces the view tao's resize event reports,
/// so the runtime reports the page. A page that has drifted from its window
/// therefore reports the drift as the window's own size.
///
/// Four callers inherited that, and ADR 0202 has what each one cost.
///
/// `outer_size()` is the NSWindow frame, which no webview touches.
/// `titleBarStyle: "Overlay"` gives the content view the whole frame, so the
/// two are one number for every window this client builds. Both halves are
/// macOS facts, and so is the client: elsewhere `outer_size` carries the
/// decorations, and this reader is where that split would go.
pub(crate) fn window_content_size(
    window: &tauri::Window,
) -> tauri::Result<tauri::PhysicalSize<u32>> {
    window.outer_size()
}

/// How far a webview may sit from filling its window before a refit is owed, in
/// physical pixels.
///
/// One, because the measurement and the correction speak different units. The
/// two sizes are compared as the physical pixels both getters answer in, and
/// the write goes out in logical points (ADR 0173). So a content view half a
/// point tall on a 2x panel round-trips to a neighbouring pixel. A zero
/// tolerance would then write on every frame of a drag.
const REFIT_TOLERANCE_PX: i64 = 1;

/// Does `webview` already cover the whole of its window?
///
/// Pure, and the reason the refit is safe to run on every resize. Every input
/// is a physical reading, the one space both getters answer in. No scale factor
/// appears, so nothing here can be off by a conversion.
///
/// `offset` is the page's top-left relative to the window's, which is (0, 0)
/// for an app window's own webview. It is judged too, because the runtime
/// stores a rate per AXIS and per CORNER: a poisoned `x_rate` insets the page
/// without changing its size.
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

/// Re-assert that an app window's own webview fills its window.
///
/// An app window is exactly one webview under its own label, and that webview
/// covers the whole window. Nothing insets it. That is an invariant the client
/// states absolutely, so the client ASSERTS it rather than trusting the runtime
/// to have kept it.
///
/// The runtime does not keep the page over the window directly. It keeps a RATE
/// per webview, the bounds last set over the window's size at that moment, and
/// re-applies `window_size * rate` on every resize. Nothing re-derives it, so
/// one wrong reading survives every later move, resize and relaunch. This
/// function used to BE that reading, through
/// `tauri::Window::inner_size()`: see [`window_content_size`], and ADR 0202.
///
/// **The honest read is what makes the rate safe.** Both sides of the runtime's
/// division now name the same thing, so the rate this records is 1.0 every
/// time. Running on every resize is therefore the repair rather than a risk:
/// each pass re-pins the rate, and a wrong one cannot outlive the next resize.
///
/// The mismatch gate is a saving, and an honest one only on a SETTLED window. A
/// live drag writes each frame, because this runs from `on_window_event`, which
/// the runtime calls before its own auto-resize. So the page read here is
/// always the previous frame's.
///
/// By webview, not webview window, per ADR 0140. This resizes a page inside a
/// window, and the window may well be hosting a URL preview. That preview keeps
/// its own bounds: it is built without `auto_resize`, so the runtime's resize
/// handler never touches it.
pub(crate) fn refit_webview(app: &tauri::AppHandle, label: &str) {
    let Some(webview) = app.get_webview(label) else {
        return;
    };
    let window = webview.window();
    let Ok(size) = window_content_size(&window) else {
        eprintln!("[Tauri] Could not read {label} to refit its webview");
        return;
    };
    // The gate. Read before the scale factor, so a settled window costs three
    // getters and no write. An unreadable page is refit rather than assumed
    // good: a needless write records a rate of 1, and skipping records the
    // defect.
    if let (Ok(page), Ok(at)) = (webview.size(), webview.position()) {
        if webview_fills_window(size, page, at) {
            return;
        }
    }
    let Ok(scale) = window.scale_factor() else {
        eprintln!("[Tauri] Could not read the scale factor of {label} to refit its webview");
        return;
    };
    // Points, through the window's OWN factor, which is what tao multiplied by
    // (ADR 0173). An unreadable one skips rather than guesses: a wrong size
    // here is the very fault this exists to undo.
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

/// The size a window nothing is remembered about is built at, in logical
/// points. The numbers `tauri.conf.json` declares for `main`, kept in step by
/// `the_default_size_is_the_declared_one`.
const DEFAULT_WIDTH_POINTS: f64 = 1024.0;
const DEFAULT_HEIGHT_POINTS: f64 = 768.0;

/// The geometry an extra window is BUILT with, in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BuildGeometry {
    width: f64,
    height: f64,
    /// `None` leaves the placing to macOS, which centres on the primary.
    position: Option<(f64, f64)>,
}

/// Where an extra window is born, given the frame its workspace remembers.
///
/// A remembered frame goes to the BUILDER, not to `place_window` afterwards.
/// tao creates the NSWindow at that content rect, so the window opens on the
/// display the frame names. It carries that display's scale factor from its
/// first frame, and wry computes the child webview's bounds from the same
/// factor. Born on the primary and moved after, it changed scale with a resize
/// still queued, and the page came out halved (ADR 0178).
///
/// No frame is File > New Window, or a workspace nothing is remembered about.
/// That takes the declared default size and no position, which leaves macOS to
/// centre it.
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

/// Counter for generating unique webview/window labels.
static WEBVIEW_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The next number for a generated label. One counter for app windows and
/// preview children together, so no two labels can collide.
pub(crate) fn next_webview_label_counter() -> u32 {
    WEBVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Label of the app window declared in `tauri.conf.json`.
///
/// It is the one window a packaged close only HIDES, so it can be reshown
/// instantly. That is why every "bring the client forward" path prefers it.
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

/// Sort key over window labels: `main` first, then the rest alphabetically.
///
/// Shared, because two places order the same window set and must agree.
/// `window_session::capture` writes the record in this order and
/// `desktop::reopen_plan` reads it back in it. The window map itself has no
/// order, so without one key the same two windows could swap places.
pub(crate) fn window_order_key(label: &str) -> (bool, &str) {
    (label != MAIN_WINDOW_LABEL, label)
}

/// Label prefix for additional top-level app windows opened via File → New
/// Window. The first window is `main` (declared in `tauri.conf.json`); each
/// extra window gets `window-<n>`. Panel preview webviews use the
/// `url-preview-<n>` prefix instead, so app-window-only setup (the app-version
/// injection in `on_page_load`) can tell the two apart.
const APP_WINDOW_PREFIX: &str = "window-";

/// True if `label` names a top-level Lucidos app window (the declared `main` or
/// a New-Window child), as opposed to a `url-preview-*` panel webview.
pub(crate) fn is_app_window(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL || label.starts_with(APP_WINDOW_PREFIX)
}

/// Start a native window drag for the calling window. `useWindowDragRegion`
/// calls this once the pointer crosses a small movement threshold, so plain
/// clicks still reach the page's own handlers. An app command rather than
/// `data-tauri-drag-region`, whose internal `plugin:window|start_dragging` IPC
/// the capability ACL denies.
#[tauri::command]
pub(crate) fn start_window_drag(window: tauri::Window) -> Result<(), String> {
    window.start_dragging().map_err(|e| format!("{e}"))
}

/// Toggle the calling window between maximized and restored. Bound to a
/// double-click on the reclaimed title-bar strip only, since the header keeps
/// its own double-click. An app command, like `start_window_drag`, so the
/// window-plugin ACL does not apply.
#[tauri::command]
pub(crate) fn toggle_window_maximize(window: tauri::Window) -> Result<(), String> {
    if window.is_maximized().map_err(|e| format!("{e}"))? {
        window.unmaximize().map_err(|e| format!("{e}"))
    } else {
        window.maximize().map_err(|e| format!("{e}"))
    }
}

/// Title the CALLING window, so the macOS Window menu names the workspace that
/// window is showing instead of listing "Lucidos" once per window.
///
/// The calling window, never `main`: two windows can sit on two workspaces, and
/// the one to retitle is the one whose page reported the name. An app command,
/// like `start_window_drag`, so the window-plugin ACL does not apply.
///
/// The title is invisible in the window itself, since `titleBarStyle: "Overlay"`
/// plus `hiddenTitle` leaves that band to the webview. Where it does show is the
/// Window menu, Mission Control and the window switcher.
#[tauri::command]
pub(crate) fn set_window_title(window: tauri::Window, title: String) -> Result<(), String> {
    window.set_title(&title).map_err(|e| format!("{e}"))
}

/// Open an additional top-level app window (File → New Window / Cmd+N) on the
/// window the user is looking at.
///
/// Every window is just another client of the same engine, which runs with
/// Postgres as a shared launchd service (see `desktop`). So all windows share
/// one workspace stack. The WKWebView crash-recovery watchdog stays scoped to
/// `main`.
///
/// **No remembered frame, deliberately.** This is a SECOND window on the
/// workspace you are already looking at, and the record holds one frame per
/// workspace. Handing it that frame would drop the new window exactly on top of
/// the one it was opened from. The declared default, centred, is the answer.
pub(crate) fn open_new_window(app: &tauri::AppHandle) -> Result<(), String> {
    open_app_window(app, new_window_url(app), None)
}

/// Build a top-level app window at `url`. The one builder every extra window
/// goes through, so a window opened for a notification tap is identical to a
/// File → New Window one: same `window-<n>` label (which is what
/// `desktop::gateway_capability` scopes IPC to), same title-bar style, same
/// pre-paint tint and traffic-light placement.
///
/// `frame` is the geometry the window's WORKSPACE was last left at, in logical
/// points. Such a window is BUILT at that frame and hidden, then shown, so it
/// never appears at the default size and jumps. It is also born on the display
/// the frame names, which is what keeps its page from rendering at half size
/// (ADR 0178). `None` takes the declared default, centred: File > New Window,
/// and a workspace nothing is remembered about.
///
/// The show is `set_visible(true)`, which is `makeKeyAndOrderFront` on macOS. So
/// a window opened by a click still arrives key, and needs no focus call of its
/// own.
fn open_app_window(
    app: &tauri::AppHandle,
    url: WebviewUrl,
    frame: Option<window_restore::Rect>,
) -> Result<(), String> {
    let counter = next_webview_label_counter();
    let label = format!("{APP_WINDOW_PREFIX}{counter}");

    // The `tauri.conf.json` window values apply only to the declared `main`
    // window, so a builder-made one repeats them or renders the default opaque
    // bar. Two are repeated here.
    //
    // `disable_drag_drop_handler` mirrors `dragDropEnabled: false`. Left on, wry
    // installs its own NSDraggingDestination handler and consumes the drag. No
    // HTML5 `dragover` or `drop` then reaches the page, so every file drop is
    // silently dead. Nothing listens for a Tauri drag-drop event, so turning it
    // off gives up nothing.
    let geometry = build_geometry(frame);
    let builder = WebviewWindowBuilder::new(app, &label, url)
        .title("Lucidos")
        .inner_size(geometry.width, geometry.height)
        .disable_drag_drop_handler();
    // The declared minimums too, which `main` takes from the config and a
    // builder-made window otherwise has none of. See `declared_min_size`: a
    // window draggable below the floor would lose its own size on restore.
    let builder = match window_restore::declared_min_size(app) {
        Some((width, height)) => builder.min_inner_size(width, height),
        None => builder,
    };
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    // A restored window is built HIDDEN and shown at the end, at its own frame
    // rather than at the default and jumping.
    //
    // The BUILDER places it, not `place_window` afterwards. The two cannot
    // drift. Both speak logical points, and tao routes each through the same
    // `window_position` flip. `titleBarStyle: "Overlay"` makes the content rect
    // the frame, so `inner_size` and `set_size` mean one thing.
    //
    // Hidden and placed are ONE decision, `geometry.position`, and the show
    // below reads the same one. Split across two tests, a window could be
    // hidden by the builder and never reach the branch that shows it.
    let builder = match geometry.position {
        Some((x, y)) => builder.position(x, y).visible(false),
        None => builder,
    };
    let placed = geometry.position.is_some();
    let window = builder.build().map_err(|e| format!("{e}"))?;
    if placed {
        // Same sanity pass `main` gets: a frame saved against a display that is
        // no longer attached must not put a window somewhere unreachable. It
        // judges real geometry here, because the builder applied the frame when
        // it created the NSWindow. tao defers a SETTER to the main queue, so a
        // clamp straight after one reads the geometry the window still has.
        window_restore::clamp_restored_geometry(app, &label);
    }
    // Tint the bar now, so it is not black for the moment before this window's
    // frontend boots and calls `set_titlebar_color`. `build()` has registered
    // the window, so `paint_title_bars` covers it.
    if let Some(color) = crate::pre_paint_title_bar_color(app) {
        crate::paint_title_bars(app, color);
    }
    // Same for the traffic lights, at the remembered bar height rather than
    // centred for the default scale.
    traffic_lights::place_all(app);
    // Last, so a restored window's first frame is already the right size, in
    // the right place, and tinted.
    // An unplaced window is BORN visible. A placed one was built hidden, so it
    // is on screen only if the show below works.
    let mut on_screen = !placed;
    if placed {
        match window.show() {
            Ok(()) => on_screen = true,
            Err(e) => eprintln!("[Tauri] Failed to show the restored window {label}: {e}"),
        }
    }
    // Recording a failed show as on screen would leave the watchdog reloading an
    // invisible page for the life of the process.
    if on_screen {
        window_screen::note_shown(&label);
    }
    // Either way this window is now on screen, which is what the session gate
    // waits for. See `window_persist::note_presented`.
    window_persist::note_presented();
    Ok(())
}

/// The URL a freshly opened app window should load. Mirrors the main window's
/// current URL once it has navigated to the gateway, so the new window lands on
/// the workspace the user is viewing. Falls back to the gateway on the stable
/// packaged port, or to the bundled entry in dev.
fn new_window_url(app: &tauri::AppHandle) -> WebviewUrl {
    // The FOCUSED window first, which is what this function's own doc promises
    // and what macOS does. Reading `main` alone opened the second window on
    // `main`'s workspace. So from any other window, a second window on the one
    // you were looking at was the single thing New Window could not give you.
    //
    // `main` stays the fallback: a tray reopen focuses nothing.
    //
    // By webview, not webview window, per ADR 0140. This reads a URL, which is
    // a page operation, and focus through `webview.window()`. Blind, it read no
    // URL off a preview-hosting window, so New Window landed on the picker
    // rather than the workspace you were on. That undid the 0.30.4 fix.
    let focused = app
        .webviews()
        .into_iter()
        .filter(|(label, _)| is_app_window(label))
        .find(|(_, webview)| webview.window().is_focused().unwrap_or(false))
        .map(|(_, webview)| webview);
    let source = focused.or_else(|| app.get_webview(MAIN_WINDOW_LABEL));
    if let Some(url) = source.and_then(|w| w.url().ok()) {
        if url.scheme() == "http" || url.scheme() == "https" {
            return WebviewUrl::External(url);
        }
    }
    if !tauri::is_dev() {
        // The same builder `desktop::launch` navigates the main window with. So
        // a New Window opened before that navigation still lands on the origin
        // `desktop::gateway_capability` pinned the ACL to.
        if let Ok(url) = desktop::gateway_url(desktop::engine_port()).parse::<tauri::Url>() {
            return WebviewUrl::External(url);
        }
    }
    WebviewUrl::App("index.html".into())
}

/// Every top-level app window as `(label, url)`, the shape both window choosers
/// take. Panel previews are left out: nothing ever targets one.
///
/// An unreadable URL reads as "not navigated", which sends that window down the
/// boot path rather than making it a target. Say so, or it is silently stranded.
fn app_window_urls(app: &tauri::AppHandle) -> Vec<(String, String)> {
    live_app_windows(app)
        .into_iter()
        .map(|window| (window.label, window.url))
        .collect()
}

/// Every top-level app window with the visibility a reopen needs too. The one
/// reader, so a window chooser and [`reopen_client`] cannot see different sets.
///
/// Enumerates WEBVIEWS rather than webview windows, per ADR 0140. Miss a window
/// here and the chooser opens a second one on a workspace that has one already.
/// A reopen builds one beside the window it could not see.
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
                label,
                url,
                // Unreadable counts as hidden, the same default
                // `visible_app_windows` takes. Showing a window that was already
                // up costs nothing; skipping one leaves it parked for good.
                visible: webview.window().is_visible().unwrap_or(false),
            }
        })
        .collect()
}

/// A composed workspace URL as a `tauri::Url`. The composer only ever emits an
/// http(s) URL, so a failure here means the origin itself was unusable.
fn parse_window_url(url: &str) -> Result<tauri::Url, String> {
    url.parse::<tauri::Url>()
        .map_err(|e| format!("could not open {url}: {e}"))
}

/// Show `workspace` in a window: what activating its row does on the packaged
/// desktop client, in the gateway picker and in the Lucidos menu's switcher.
///
/// Three outcomes, and `window_target::choose_workspace_target` picks between
/// them: focus the window already on the workspace, point the calling window at
/// it, or open a new one.
///
/// A command exists because the web answer is unavailable here. WKWebView drops
/// `window.open`: wry installs a new-window delegate only for a builder that
/// calls `.on_new_window()`, and no app window does.
///
/// It takes a SLUG, never a URL, and composes the URL itself. Every `window-*`
/// webview holds the full IPC grant on the gateway origin (ADR 0028). A URL
/// chosen by the page would be the page choosing what loads there.
///
/// `landing` names a view inside the workspace, validated the same way and for
/// the same reason. It reaches a peer's notifications rather than only the
/// default view. It also navigates a window already on the workspace, rather
/// than merely fronting it. The caller arrives as a `Webview` (ADR 0140),
/// because only a webview carries the URL this needs.
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
    let windows = app_window_urls(&app);
    // The WINDOW's label, not the webview's. The two agree for an app window,
    // and `app_window_urls` keys by window label, so ask in those terms.
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
            // By webview, not webview window, per ADR 0140. The window this
            // chooses is the likeliest of all to be hosting a URL preview.
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
            // At the size and place this workspace was last left, which is the
            // whole reason the record keeps geometry after a window closes
            // (ADR 0123). Reached only when NO window is on the workspace, so
            // the remembered frame cannot land on top of the window it came
            // from. That is also why File > New Window takes no frame.
            let frame = window_persist::remembered_frame(&url);
            open_app_window(&app, WebviewUrl::External(parse_window_url(&url)?), frame)
        }
    }
}

/// Settle `main`'s geometry: place the frame it is owed, or judge the one it
/// already wears.
///
/// Never both. A placement is deferred to tao's main queue. A clamp issued
/// behind one therefore reads the geometry that placement is about to replace,
/// and can correct the rect it is losing (ADR 0202). A chosen frame goes
/// through [`window_restore::sanitized_frame`] before it is written instead.
///
/// The one settler, shared by the startup show and by [`reopen_client`], so the
/// two cannot come to different arrangements for the same window.
pub(crate) fn settle_main_geometry(app: &tauri::AppHandle, frame: Option<window_restore::Rect>) {
    match frame {
        Some(frame) => {
            let frame = window_restore::sanitized_frame(app, MAIN_WINDOW_LABEL, frame);
            window_persist::size_main_window_for_its_workspace(app, frame);
        }
        None => window_restore::clamp_restored_geometry(app, MAIN_WINDOW_LABEL),
    }
    MAIN_GEOMETRY_SETTLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Has this launch settled `main`'s geometry yet?
///
/// The startup show does it, and a LOGIN START never reaches one: it comes up
/// menu-bar-only, so neither racer can claim the show. The first reopen settles
/// it instead, which is what this latch bounds to ONCE.
///
/// Unbounded, every later tray click would re-place a window the user had since
/// dragged. Inside the save debounce it would land on the pre-drag rect, so the
/// window would visibly snap back.
static MAIN_GEOMETRY_SETTLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The frame a reopen owes `main`, or `None` to judge what it already wears.
///
/// Pure, so the precedence is testable without a window. `navigated` is the
/// plan's answer: `Some(frame)` when this reopen is pointing `main` at a
/// workspace, where the inner option is that workspace's remembered frame.
///
/// An adrift `main` takes what it is being navigated to. One already on its
/// workspace owes nothing once the geometry is settled, because every frame
/// after that is where the user put it. Unsettled, it takes `remembered`: see
/// [`MAIN_GEOMETRY_SETTLED`] for the launch that leaves it so.
fn main_frame_owed(
    navigated: Option<Option<window_restore::Rect>>,
    settled: bool,
    remembered: Option<window_restore::Rect>,
) -> Option<window_restore::Rect> {
    match navigated {
        Some(frame) => frame,
        None if settled => None,
        None => remembered,
    }
}

/// [`main_frame_owed`]'s three inputs, read off this process.
fn main_frame_owed_now(
    live: &[desktop::LiveWindow],
    plan: &desktop::ReopenPlan,
) -> Option<window_restore::Rect> {
    let settled = MAIN_GEOMETRY_SETTLED.load(std::sync::atomic::Ordering::SeqCst);
    main_frame_owed(
        plan.navigate_main.as_ref().map(|planned| planned.frame),
        settled,
        // Skipped when settled, so a tray click costs no file read.
        (!settled)
            .then(|| {
                live.iter()
                    .find(|window| window.label == MAIN_WINDOW_LABEL)
                    .and_then(|window| window_persist::remembered_frame(&window.url))
            })
            .flatten(),
    )
}

/// Reopen the extra windows this launch owes, at the frames they were left at.
///
/// `main` takes the first restored workspace and is navigated by the caller, so
/// this covers everything after it. The URL is composed by the caller from a
/// validated slug, never read off the record.
pub(crate) fn restore_extra_windows(app: &tauri::AppHandle, windows: &[desktop::PlannedWindow]) {
    for window in windows {
        let url = &window.url;
        let Ok(parsed) = url.parse::<tauri::Url>() else {
            eprintln!("[Tauri] Cannot restore a window on an unparseable URL: {url}");
            continue;
        };
        if let Err(e) = open_app_window(app, WebviewUrl::External(parsed), window.frame) {
            eprintln!("[Tauri] Failed to restore a window on {url}: {e}");
        }
    }
}

/// Hide every Lucidos client window. Best-effort, since a hide failure must not
/// abort the uninstall. Window messages are proxied to the main event loop, so
/// this is safe from the dialog callback thread.
///
/// By window, not webview window, per ADR 0140. No filter is needed: a preview
/// child is a webview and never a window, so this map holds app windows only.
pub(crate) fn hide_all_windows(app: &tauri::AppHandle) {
    for (label, window) in app.windows() {
        let _ = window.hide();
        window_screen::note_hidden(&label);
    }
}

/// Bring the CALLING page's own window to the front.
///
/// Exposed to the page for the one flow that finishes somewhere else: an OAuth
/// authorization the user completes in a browser. Without it they approve the
/// consent screen and are left staring at the callback tab.
///
/// It fronts `window`, and deliberately NOT [`show_main_window`], which targets
/// `main` and builds a new one when it is gone. Each app window can sit on its
/// own workspace, so fronting `main` would raise a workspace the user had not
/// asked for. The caller's window exists by construction, so there is no
/// create-a-window branch. Leaving menu-bar-only first is kept, since the
/// calling page may live in a hidden window and `Accessory` fronts nothing.
///
/// Still NOT a general "focus me" the page may call whenever it likes. Its one
/// caller fires in the page that OPENED the authorization URL, seconds after the
/// user's own click, and once. Keep new callers to that shape.
#[tauri::command]
pub(crate) fn focus_calling_window(app: tauri::AppHandle, window: tauri::Window) {
    // Restore `Regular` BEFORE showing: the AppKit `Accessory` to `Regular`
    // transition otherwise leaves the app behind other apps with an unclickable
    // menu bar.
    activation::set_menu_bar_only(&app, false);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    activation::activate_app_frontmost();
    window_screen::note_shown(window.label());
    // `set_focus()` also fires `WindowEvent::Focused(true)`, but emit explicitly
    // so the reshow is deterministic regardless of event timing.
    emit_window_active(&app, window.label(), true);
    // A window just reached the screen, which is the whole of what the session
    // gate latches on. Every path that shows one says so. See `window_persist::note_presented`.
    window_persist::note_presented();
}

/// Report whether the CALLING page's own window is ACTIVE: focused and
/// on-screen.
///
/// The frontend pulls this at startup to SEED its `native-window-active` cache
/// before registering the event listener. Tauri does not replay the transition
/// events to a listener that registers after the fact, and the cache defaults to
/// `true`. Without the seed, a freshly loaded page that is really backgrounded
/// keeps that default and pongs the device as active. The engine then suppresses
/// the OS push into an invisible in-app toast.
///
/// **It reads `window`, never `main`.** Each app window can sit on its own
/// workspace, and the transitions it seeds ahead of are `emit_to` one label. A
/// seed off another window therefore answers about a page that is not this one.
/// Reading `main` is how a backgrounded second window seeded itself active from
/// a focused first one. It then pongs `is_active: true` and its workspace's push
/// is suppressed, the `Any`-listener symptom reached one route further back.
///
/// Any state read that fails resolves to the SAFE direction, inactive, so an
/// uncertain seed surfaces the banner rather than suppressing it.
#[tauri::command]
pub(crate) fn get_native_window_active(window: tauri::Window) -> bool {
    let focused = window.is_focused().unwrap_or(false);
    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    focused && visible && !minimized
}

/// Bridge the native window's ACTIVE state, focused and on-screen, to that
/// window's webview as a `native-window-active` event.
///
/// The embedded WKWebView cannot observe macOS `orderOut:`. A window dismissed
/// to the tray keeps `visibilityState` visible and `hasFocus()` true, so the
/// page cannot tell in-use from trayed on its own. The frontend feeds this into
/// `isPageActive()`, so a non-active client gets the OS banner rather than a
/// suppressed in-app toast. Targeted to one window, so a secondary New-Window
/// client keeps its own state.
pub(crate) fn emit_window_active(app: &tauri::AppHandle, label: &str, active: bool) {
    let _ = app.emit_to(label, "native-window-active", active);
}

/// How many top-level app windows the user can actually see. `excluding` skips a
/// window that is on its way out but might still be listed. An unreadable
/// visibility counts as hidden: that keeps the tray and the reopen path from
/// leaving the client with nothing on screen.
///
/// By window, not webview window, per ADR 0140. This is the louder half of the
/// park pair above. Counting a preview-hosting window as gone took the client
/// to `Accessory` with a window still up: no Dock icon, no Cmd-Tab entry, and
/// an app menu that cannot be clicked.
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

/// Drop the client to menu-bar-only IFF no app window is left visible. Closing
/// the LAST window removes the app from the Dock and Cmd+Tab, while closing one
/// of several leaves it a normal Dock app.
pub(crate) fn enter_menu_bar_only_if_no_windows(app: &tauri::AppHandle, excluding: Option<&str>) {
    if activation::should_be_menu_bar_only(visible_app_windows(app, excluding)) {
        activation::set_menu_bar_only(app, true);
    }
}

/// Park the whole client in the menu-bar tray. HIDES every app window, then
/// drops to menu-bar-only. The launchd services are untouched, and the only full
/// teardown is [`quit_lucidos`].
///
/// Hidden, never destroyed, and that is what [`reopen_client`] gives back. It is
/// also what keeps the window session honest across a park: no `Destroyed`
/// fires, so nothing re-captures a shrunken window set, and a relaunch after a
/// park still restores the arrangement. `main` had this treatment alone, for the
/// reopen speed and the page state it preserves. A secondary window earned the
/// same the moment the session gave it a workspace identity (ADR 0123).
///
/// Packaged only. Dev has no always-on service and no tray, so hiding and going
/// `Accessory` would strand the window with no way to reopen it. Dev therefore
/// closes the windows instead, matching the default close-quits behavior.
/// By window, not webview window, per ADR 0140. Closing and hiding are window
/// operations, and the blind flavour skipped whichever window had a preview
/// open. That was always `main`, so the Cmd-Q park left it on screen.
pub(crate) fn close_all_to_tray(app: &tauri::AppHandle) {
    if tauri::is_dev() {
        for (label, window) in app.windows() {
            if is_app_window(&label) {
                let _ = window.close();
            }
        }
        return;
    }
    // The plugin's exit-time write never runs, because we hide rather than
    // exit, so this is the moment to remember size and position. Taken BEFORE
    // the loop, while every window can still report its own geometry.
    window_persist::persist_windows(app);
    for (label, window) in app.windows() {
        if is_app_window(&label) {
            let _ = window.hide();
            window_screen::note_hidden(&label);
            emit_window_active(app, &label, false);
        }
    }
    enter_menu_bar_only_if_no_windows(app, None);
}

/// Show and focus the main window, standing a fresh one in when it is gone.
///
/// ONE window, deliberately. It backs a native-notification tap with no window
/// to aim at, and the retry path an uninstall failure leaves the user on. The
/// tray's "Open Lucidos" and the Dock click want the whole arrangement instead,
/// and go through [`reopen_client`].
///
/// The stand-in is a `window-<n>`, not `main`: only tauri's config declares that
/// label, and nothing can rebuild it. So this reads "a window" rather than "the
/// main window", and a second call after a stand-in builds a second one. The
/// packaged client never reaches it, since `main`'s close is prevented.
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    // By window, not webview window, per ADR 0140. Answer "gone" for a window
    // that merely has a URL preview open and a banner tap builds a SECOND one.
    if app.get_window(MAIN_WINDOW_LABEL).is_some() {
        front_window(app, MAIN_WINDOW_LABEL);
        return;
    }
    // Gone, so stand one in. Leaving menu-bar-only first for the reason
    // `front_window` does: `Accessory` cannot front the new window either.
    activation::set_menu_bar_only(app, false);
    if let Err(e) = open_new_window(app) {
        eprintln!("[Tauri] Failed to open window: {e}");
    }
}

/// Bring the client back, with the arrangement the user left.
///
/// What the tray's "Open Lucidos" and a Dock click both mean. It shows every
/// parked window, builds anything the record names that this process no longer
/// holds, and fronts `main` last so it ends on top.
///
/// Deliberately NOT what a notification tap does. A tap names one workspace and
/// `route_native_tap` fronts the window on it (ADR 0123). Raising the whole desk
/// over the user's work is not what tapping one banner asked for.
pub(crate) fn reopen_client(app: &tauri::AppHandle) {
    // `Accessory` cannot front a window and leaves the app menu unclickable, so
    // the policy goes back BEFORE anything is shown. Same order `front_window`
    // takes, and for the same reason.
    activation::set_menu_bar_only(app, false);
    let live = live_app_windows(app);
    let urls: Vec<(String, String)> = live
        .iter()
        .map(|window| (window.label.clone(), window.url.clone()))
        .collect();
    // Prefer an origin a window is actually on, so a client reached over
    // something other than the stable loopback URL still targets itself. The
    // same fallback `route_native_tap` takes.
    let origin = notifications::gateway_origin(&urls)
        .map(str::to_string)
        .unwrap_or_else(|| desktop::gateway_url(desktop::engine_port()));
    let plan = desktop::reopen_plan(&live, &window_persist::readable_window_session(), &origin);

    // Before the show, so an adrift `main` does not flash the picker on its way
    // to the workspace it is owed, and so it is already the right size when it
    // lands.
    if let Some(planned) = &plan.navigate_main {
        desktop::navigate_main_window(app, &planned.url);
    }
    // The same decision the startup show makes, and for the same reason: either
    // the client chooses `main`'s frame, or it judges the one the window-state
    // plugin restored. Never both, since a clamp issued behind a placement
    // reads the geometry that placement is about to replace.
    settle_main_geometry(app, main_frame_owed_now(&live, &plan));
    // No `native-window-active` here: these land on screen unfocused, and a
    // page that believes it is active suppresses the OS banner for a toast
    // nobody is looking at. `front_window` emits for the one that does
    // activate, and a `Focused` event covers whichever the user clicks next.
    for label in &plan.show {
        // By window, not webview window, per ADR 0140. `live_app_windows` sees
        // a preview-hosting window, so a blind lookup here would plan to show
        // one and then fail to find it, leaving it parked.
        if let Some(window) = app.get_window(label) {
            let _ = window.unminimize();
            match window.show() {
                Ok(()) => window_screen::note_shown(label),
                Err(e) => eprintln!("[Tauri] Failed to show the parked window {label}: {e}"),
            }
        }
    }
    restore_extra_windows(app, &plan.build);
    match &plan.front {
        Some(label) => front_window(app, label),
        // No window existed to front, but the build just made some. They are
        // already on screen, so they only need the app brought forward.
        // Falling through to `show_main_window` would add one more on top.
        None if !plan.build.is_empty() => activation::activate_app_frontmost(),
        // Nothing at all to come back to, so make one.
        None => show_main_window(app),
    }
}

/// Show and focus one specific app window. The shared body of every "bring a
/// window forward" path.
///
/// Leaving menu-bar-only comes FIRST: the `Regular` activation policy has to be
/// back before the window is fronted, or the app menu is unclickable. For the
/// same reason `set_focus` alone is not enough and the app is activated
/// frontmost explicitly. `native-window-active` is emitted explicitly too, so
/// the reshow is deterministic regardless of event timing.
fn front_window(app: &tauri::AppHandle, label: &str) {
    activation::set_menu_bar_only(app, false);
    // Sanitise a window that is about to REACH the screen, and this is the path
    // that needs it most. A login start comes up menu-bar-only, so it never
    // reaches `show_startup_window` and its `main` carries whatever the
    // window-state plugin restored, unjudged. A banner tap is then the first
    // thing to put that window up. A healthy rect makes this a no-op (ADR 0193).
    //
    // Gated on the window being off screen, which is the whole difference
    // between judging a restore and moving a window under the user. Fronting
    // also focuses one that is already up, from the switcher. A window parked
    // with a sliver of title bar is the user's to leave there, and ADR 0173
    // refused to widen the clamp for that.
    if !window_screen::shown_by_client(label) {
        window_restore::clamp_restored_geometry(app, label);
    }
    // By window, not webview window, per ADR 0140. Fronting is a pure window
    // operation, and the lookup must survive a URL preview open in the target.
    if let Some(window) = app.get_window(label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        activation::activate_app_frontmost();
        window_screen::note_shown(label);
        emit_window_active(app, label, true);
        // A login-started client shows nothing until the user asks, and this is
        // where they ask. From here its window set is worth recording.
        window_persist::note_presented();
    }
}

/// Bring forward (or create) the window a native banner tap belongs in, given
/// the workspace that RAISED the banner.
///
/// One packaged process fronts the gateway, and each app window can sit on its
/// own workspace (ADR 0014). So "the window that is frontmost" and "the
/// workspace the tap came from" are unrelated. The decision lives here because
/// only this process can see every window, read what each is pointed at, and
/// open one.
///
/// Returns the label of an already-loaded window, to send the warm
/// `native-notification-tapped` wake to. `None` means the target is a page
/// about to load: a fresh page runs the startup drain itself, and an `emit`
/// into a webview mid-navigation is dropped.
///
/// The caller must have stashed the tap BEFORE calling this. Showing or focusing
/// a window fires that page's `focus` / `visibilitychange` drains, and a drain
/// that runs first finds nothing.
#[cfg(target_os = "macos")]
pub(crate) fn route_native_tap(app: &tauri::AppHandle, owner: Option<&str>) -> Option<String> {
    let windows = app_window_urls(app);

    // Prefer an origin a window is actually on, so a client reached over
    // something other than the stable loopback URL still targets itself.
    let origin = notifications::gateway_origin(&windows)
        .map(str::to_string)
        .unwrap_or_else(|| desktop::gateway_url(desktop::engine_port()));

    match notifications::choose_tap_target(&windows, owner, &origin) {
        notifications::TapTarget::Focus(label) => {
            front_window(app, &label);
            Some(label)
        }
        notifications::TapTarget::Navigate { label, url } => {
            // By webview, not webview window, per ADR 0140. A blind lookup
            // fronts the target on the page it was already on. The tap is then
            // stranded in the stash, waiting for a page that never routes it.
            match (app.get_webview(&label), url.parse::<tauri::Url>()) {
                (Some(window), Ok(parsed)) => {
                    if let Err(e) = window.navigate(parsed) {
                        eprintln!("[Tauri] Failed to point {label} at {url}: {e}");
                    }
                }
                // Never silent: the window would come forward on the wrong page
                // and the tap would sit unroutable in the stash.
                _ => eprintln!("[Tauri] Cannot point {label} at {url}: no such window / bad URL"),
            }
            front_window(app, &label);
            None
        }
        notifications::TapTarget::NewWindow { url } => {
            // The remembered frame, for the reason the row-activation arm of
            // `show_workspace_window` takes it: a tap on a banner from a
            // workspace with no window is that workspace being reopened.
            let frame = window_persist::remembered_frame(&url);
            match url.parse::<tauri::Url>() {
                Ok(parsed) => {
                    activation::set_menu_bar_only(app, false);
                    if let Err(e) = open_app_window(app, WebviewUrl::External(parsed), frame) {
                        eprintln!("[Tauri] Failed to open a window for {url}: {e}");
                    }
                    activation::activate_app_frontmost();
                }
                Err(e) => eprintln!("[Tauri] Bad tap target URL {url}: {e}"),
            }
            None
        }
        notifications::TapTarget::LaunchInto { url } => {
            // `desktop::launch` is still waiting on the gateway and owns the
            // main window's first navigation; aim it rather than race it.
            desktop::set_launch_target(url);
            show_main_window(app);
            None
        }
        notifications::TapTarget::MainWindow => {
            show_main_window(app);
            // An unattributed tap may be taken by any page, and `main` is the one
            // just fronted. If it had to be recreated instead, the fresh page's
            // startup drain is the trigger.
            //
            // By window, not webview window, per ADR 0140. Answer `None` for a
            // live `main` and the warm wake is skipped for no reason.
            app.get_window(MAIN_WINDOW_LABEL)
                .map(|_| MAIN_WINDOW_LABEL.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame the reported window came up wrong at, remembered on the 2.0
    /// Retina panel while the 1.0 ultrawide was primary.
    fn reported_frame() -> window_restore::Rect {
        window_restore::Rect {
            x: 1763,
            y: 1473,
            width: 1829,
            height: 1084,
        }
    }

    // ── Where a window is born ───────────────────────────────────────────────

    // The fix. A remembered frame reaches the BUILDER, so the window opens on
    // the display that frame names. Built at the default and moved after, it
    // changed scale factor with a resize still queued. The runtime then sized
    // the page at half the window.
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

    // File > New Window, and a workspace nothing is remembered about. No
    // position, so macOS centres it on the primary.
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

    // The default is declared once, in the config the clamp also reads. Drift
    // between the two would build a window the clamp then judges as corrupt.
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

    // ── The order a placement goes out in ────────────────────────────────────

    // The other half of the fix, and the half no other test can see. tao mints
    // its resize event in physical pixels, and the runtime divides at drain
    // time. A move queued between the two is read at the wrong factor, and
    // moving first leaves nothing to straddle.
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

    // ── Whether the page still fills its window ──────────────────────────────

    fn size(width: u32, height: u32) -> tauri::PhysicalSize<u32> {
        tauri::PhysicalSize::new(width, height)
    }

    fn at(x: i32, y: i32) -> tauri::PhysicalPosition<i32> {
        tauri::PhysicalPosition::new(x, y)
    }

    /// The reported window, measured off the live accessibility tree. The
    /// runtime held a rate of 1.6786 for it. So the page stayed 1.68 times the
    /// window's width, and nothing the user could do cleared it.
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

    /// The tolerance earns its keep here. The write goes out in points and the
    /// reading comes back in pixels, so a neighbouring pixel is a round trip
    /// rather than a fault. Refitting on it would write on every frame of a
    /// drag, which is the one thing the gate exists to avoid.
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

    /// The runtime keeps a rate per corner as well as per axis, so a page can
    /// be the right size and still be inset. Judging the size alone would leave
    /// that one uncorrected.
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

    // ── What frame a reopen owes `main` ──────────────────────────────────────

    fn frame() -> window_restore::Rect {
        window_restore::Rect {
            x: 573,
            y: 30,
            width: 3267,
            height: 1410,
        }
    }

    /// An adrift `main` is being pointed somewhere, so it takes that
    /// workspace's frame whatever this launch has done before.
    #[test]
    fn a_navigated_main_takes_the_frame_it_is_navigating_to() {
        for settled in [false, true] {
            assert_eq!(
                main_frame_owed(Some(Some(frame())), settled, None),
                Some(frame()),
                "settled {settled}"
            );
            // A workspace nothing is remembered about is navigated with no
            // frame, and falls through to the clamp.
            assert_eq!(main_frame_owed(Some(None), settled, Some(frame())), None);
        }
    }

    /// The login start. Nothing settled `main`, so the first reopen owes it the
    /// frame its workspace remembers.
    #[test]
    fn an_unsettled_main_takes_the_frame_its_workspace_remembers() {
        assert_eq!(main_frame_owed(None, false, Some(frame())), Some(frame()));
        assert_eq!(main_frame_owed(None, false, None), None);
    }

    /// The regression this bounds. Every reopen after the first finds `main`
    /// where the user left it. Re-placing it there would snap a dragged window
    /// back to whatever the record last caught.
    #[test]
    fn a_settled_main_is_left_exactly_where_it_is() {
        assert_eq!(main_frame_owed(None, true, Some(frame())), None);
    }

    #[test]
    fn is_app_window_distinguishes_app_windows_from_panel_previews() {
        // The declared main window and New-Window children are app windows…
        assert!(is_app_window("main"));
        assert!(is_app_window("window-0"));
        assert!(is_app_window("window-42"));
        // …while panel URL previews and anything else are not.
        assert!(!is_app_window("url-preview-3"));
        assert!(!is_app_window("lucidos-tray"));
        assert!(!is_app_window(""));
    }
}
