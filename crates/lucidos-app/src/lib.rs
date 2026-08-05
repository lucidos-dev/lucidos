use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Mutex;
use std::time::Instant;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::PageLoadEvent;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

mod desktop;
mod device_id_store;
mod mobile;
mod notifications;
mod updater;

/// Headless launchd entry point — `Lucidos --service` (see `desktop::run_service`).
/// Boots the bundled Postgres + engine and supervises them with no window. The
/// caller (`main`) routes the process here before any Tauri init.
pub fn run_service() -> i32 {
    desktop::run_service()
}

/// Format a Safari-like user-agent string for the given Safari version.
/// WKWebView's default UA omits the `Version/X.Y Safari/605.1.15` suffix,
/// making Google Docs (and others) think it's an unsupported browser. Pure
/// (no IO) so the format is unit-testable independently of the `defaults` probe.
fn safari_ua(version: &str) -> String {
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
         AppleWebKit/605.1.15 (KHTML, like Gecko) \
         Version/{version} Safari/605.1.15"
    )
}

/// Build a Safari-like user-agent from the actual system Safari version
/// (falling back to `18.0` when the `defaults` probe fails). Cached via
/// `OnceLock` so the `defaults` process only spawns once.
fn safari_user_agent() -> &'static str {
    static UA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    UA.get_or_init(|| {
        let safari_version = std::process::Command::new("defaults")
            .args([
                "read",
                "/Applications/Safari.app/Contents/Info.plist",
                "CFBundleShortVersionString",
            ])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "18.0".to_string());

        safari_ua(&safari_version)
    })
}

fn open_in_default_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let cmd = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "linux")]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();

    if let Err(e) = cmd {
        eprintln!("[Tauri] Failed to open URL in browser: {e}");
    }
}

/// Channel for receiving page content extracted from the panel webview.
struct PanelContentChannel(Mutex<Option<std::sync::mpsc::Sender<(String, String)>>>);

/// Tracks the label of the currently active panel webview.
struct PanelWebview(Mutex<Option<String>>);

/// Sender that wakes the dock-badge loop (`desktop::launch`) for an immediate
/// recompute. The frontend's `nudge_dock_badge` command sends on it when a
/// notification SSE arrives, so the macOS dock badge updates the instant a
/// notification is read (in-app or from another device) instead of on the next
/// poll tick. The receiver lives in the badge thread (macOS, packaged); a send
/// is a harmless no-op when there's no consumer (dev / non-macOS).
struct DockBadgeNudge(Mutex<std::sync::mpsc::Sender<()>>);

/// Tracks the JS heartbeat for WKWebView crash recovery. WKWebView's content
/// process can be terminated by macOS under memory pressure, leaving a white
/// screen. The JS side calls `heartbeat` every 15s; if we don't hear from it for
/// [`HEARTBEAT_TIMEOUT`], the watchdog reloads the webview.
struct LastHeartbeat {
    /// When the most recent heartbeat arrived.
    at: Mutex<Instant>,
    /// Monotonic count of heartbeats received. The timestamp alone cannot tell
    /// "the page came back after my reload and then died again" from "the page
    /// has never beaten at all", because the watchdog resets the timestamp
    /// itself on every reload. The count can, and that distinction is what stops
    /// a pointless reload from repeating forever.
    count: AtomicU64,
}

/// How long the JS heartbeat may go silent before the watchdog treats the
/// WKWebView content process as crashed and reloads it. The page heartbeats
/// every 15s, so 60s is four missed beats.
const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How often the watchdog re-checks. Well under [`HEARTBEAT_TIMEOUT`] so a
/// genuine crash is caught promptly.
const WATCHDOG_TICK: std::time::Duration = std::time::Duration::from_secs(15);

/// Cap on the backoff doublings in [`reload_threshold`]: 60s << 5 ≈ 32 minutes.
/// Bounded rather than unbounded so a page that recovers on its own (say the
/// gateway finally comes up) is still noticed within a useful window.
const MAX_RELOAD_BACKOFF_DOUBLINGS: u32 = 5;

/// How long the heartbeat may go silent before the next reload, given how many
/// consecutive reloads have already failed to bring it back.
///
/// A reload that produces no heartbeat did not fix anything, and repeating it on
/// the base interval is how the 2.11 ACL regression turned into 6232 silent
/// reloads (one per minute for weeks). Backing off — rather than giving up —
/// kills the thrash while still recovering if the cause was temporary, e.g. the
/// page failing to load because the gateway was briefly down.
fn reload_threshold(futile_reloads: u32) -> std::time::Duration {
    HEARTBEAT_TIMEOUT * 2u32.pow(futile_reloads.min(MAX_RELOAD_BACKOFF_DOUBLINGS))
}

/// What the watchdog decided to do on one tick.
#[derive(Debug, PartialEq, Eq)]
struct ReloadDecision {
    /// The previous reload produced no heartbeat at all, so this one is very
    /// unlikely to help either — something other than a content-process crash is
    /// wrong (an ACL-rejected IPC bridge, for instance).
    futile: bool,
    /// How long the heartbeat may now go silent before the watchdog tries again.
    next_threshold: std::time::Duration,
}

/// The watchdog's state machine, kept pure so the escalation is unit-testable
/// without a webview or a 32-minute wall clock.
#[derive(Debug, Default)]
struct ReloadWatchdog {
    /// Heartbeat count observed at the last reload; `None` before the first one,
    /// so the first reload is never judged futile.
    heartbeats_at_last_reload: Option<u64>,
    /// Consecutive reloads after which the page still never beat.
    futile_reloads: u32,
}

impl ReloadWatchdog {
    /// One watchdog tick. `Some(..)` means reload now; the caller must then reset
    /// the heartbeat timestamp so the next threshold is measured from the reload.
    fn on_tick(
        &mut self,
        silent_for: std::time::Duration,
        heartbeats: u64,
    ) -> Option<ReloadDecision> {
        if silent_for <= reload_threshold(self.futile_reloads) {
            return None;
        }
        let futile = self.heartbeats_at_last_reload == Some(heartbeats);
        self.futile_reloads = if futile {
            self.futile_reloads.saturating_add(1)
        } else {
            0
        };
        self.heartbeats_at_last_reload = Some(heartbeats);
        Some(ReloadDecision {
            futile,
            next_threshold: reload_threshold(self.futile_reloads),
        })
    }
}

/// Window state the app persists/restores via `tauri-plugin-window-state`.
/// Deliberately EXCLUDES `VISIBLE` and `DECORATIONS`:
/// - `VISIBLE`: the packaged client *hides* (never closes) its window — the
///   always-on launchd service keeps the process resident. A geometry flush taken
///   while the window is hidden would persist `visible: false`, and the plugin
///   would then restore the window hidden on the next launch (the user opens the
///   app and sees nothing). Leaving `VISIBLE` out means the plugin never forces
///   the window hidden on restore; the config-declared `main` window stays visible.
/// - `DECORATIONS`: toggling decorations on macOS rebuilds the NSWindow style mask
///   and can drop our `titleBarStyle: "Overlay"` + hidden-title configuration,
///   turning the reclaimed title-bar band back into an opaque bar after restore.
///
/// What's left — size, position, maximized, fullscreen — is exactly what the user
/// expects remembered, including which screen (the plugin restores position only
/// when a currently-connected monitor still contains the saved rect).
fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
}

/// How long the window must sit still (no move/resize) before the debounced
/// background flush writes `.window-state.json`. Short enough that a quick
/// move-then-relaunch is remembered, long enough that a drag doesn't thrash the
/// disk on every intermediate `Moved`/`Resized` event.
const GEOMETRY_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(600);

/// Coordinates the debounced geometry flush. The window-state plugin only writes
/// to disk on `RunEvent::Exit`, which the packaged client deliberately never
/// reaches (closing the window hides it; exit is prevented while the launchd
/// service runs). So without this, a moved/resized window is remembered only
/// in-memory and is lost on the next client launch. `Moved`/`Resized` events mark
/// `dirty` + stamp `last_change`; a background thread flushes once the window has
/// been quiet for [`GEOMETRY_SAVE_DEBOUNCE`] (see [`should_persist_geometry`]).
struct GeometrySaver {
    dirty: AtomicBool,
    last_change: Mutex<Instant>,
}

/// Whether the debounced flush should run now: there is unsaved geometry (`dirty`)
/// and the window has been quiet at least [`GEOMETRY_SAVE_DEBOUNCE`]. Pure so the
/// debounce threshold is unit-testable without the background thread.
fn should_persist_geometry(dirty: bool, since_last_change: std::time::Duration) -> bool {
    dirty && since_last_change >= GEOMETRY_SAVE_DEBOUNCE
}

/// Persist window geometry, forcing the work onto the MAIN thread.
///
/// `tauri-plugin-window-state::save_window_state` holds an internal cache lock
/// while it reads each window's live geometry (`inner_size` / `outer_position` /
/// `is_maximized` / …). Off the main thread those getters block on a round-trip
/// to the event loop — so a worker-thread save holds the cache lock across a wait
/// for the main thread, while the main thread (delivering a `Moved`/`Resized`/
/// `CloseRequested` to the plugin's own handlers) blocks taking that same lock:
/// a full-UI deadlock. The plugin avoids it by only saving from `RunEvent::Exit`
/// (after the loop stops). Callers NOT already on the main thread (the debounce
/// thread, async commands) must route through here; the getters then resolve
/// inline and the lock is never held across a cross-thread wait. Fire-and-forget
/// — the save runs on the next main-loop turn.
fn persist_window_state_on_main(app: &tauri::AppHandle) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Err(e) = handle.save_window_state(window_state_flags()) {
            eprintln!("[Tauri] Failed to persist window state: {e}");
        }
    }) {
        eprintln!("[Tauri] Failed to schedule window-state save: {e}");
    }
}

/// Counter for generating unique webview/window labels.
static WEBVIEW_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Set true only by the explicit full-teardown path ("Quit & Stop Background
/// Service" — `quit_lucidos`) so the `ExitRequested` handler lets that
/// `app.exit(0)` through. Otherwise that handler prevents the auto-exit a
/// last-window close would trigger, keeping the client resident in the menu bar
/// while the always-on launchd service runs untouched. Packaged only.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Label prefix for additional top-level app windows opened via File → New
/// Window. The first window is `main` (declared in `tauri.conf.json`); each
/// extra window gets `window-<n>`. Panel preview webviews use the
/// `url-preview-<n>` prefix instead, so app-window-only setup (the app-version
/// injection in `on_page_load`) can tell the two apart.
const APP_WINDOW_PREFIX: &str = "window-";

/// Default macOS window background tint, applied before the frontend reports the
/// active theme. It's the dark-theme header-top blue — the first stop of
/// `--header-gradient` in `crates/lucidos-app/src/styles/global/base.css`; dark
/// is the app's default theme. Under `titleBarStyle: "Overlay"` the webview owns
/// the full window height and paints the reclaimed title-bar band itself (the
/// `.titlebar-strip` element); this NSWindow background is the pre-paint /
/// behind-the-webview fallback so that band reads blue, not black, before the
/// page paints. The frontend's `applyTheme` refines it to the exact per-theme
/// blue via the `set_titlebar_color` command.
const TITLE_BAR_DEFAULT_COLOR: &str = "#15549e";

/// JS appended to every app window's startup injection. On macOS it sets the
/// `--titlebar-inset` CSS variable (the macOS standard title-bar height) before
/// the page paints, so under `titleBarStyle: "Overlay"` the reclaimed title-bar
/// band — drawn by the `.titlebar-strip` element, which sizes to
/// `var(--titlebar-inset, 0px)` — appears with no layout shift, while the header
/// content below keeps its position. Empty on every other platform and the web
/// build, where there is no native title bar: `--titlebar-inset` stays unset
/// (0px) and the strip collapses to nothing.
fn titlebar_inset_script() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "if(document.documentElement)\
         document.documentElement.style.setProperty('--titlebar-inset','28px');"
    }
    #[cfg(not(target_os = "macos"))]
    {
        ""
    }
}

/// Get the main window. Tries get_window first, falls back to get_webview_window.
fn get_main_window(app: &tauri::AppHandle) -> Option<tauri::Window> {
    if let Some(w) = app.get_window("main") {
        return Some(w);
    }
    app.get_webview_window("main")
        .map(|ww| ww.as_ref().window().clone())
}

/// True if `label` names a top-level Lucidos app window (the declared `main` or
/// a New-Window child), as opposed to a `url-preview-*` panel webview.
fn is_app_window(label: &str) -> bool {
    label == "main" || label.starts_with(APP_WINDOW_PREFIX)
}

/// Paint the macOS window background of every top-level app window the given
/// color. Sets the WINDOW-layer background only (`Window::set_background_color`),
/// never the webview's — so the page background is never tinted (no load flash).
/// Under `titleBarStyle: "Overlay"` this is the pre-paint / behind-the-webview
/// fallback for the reclaimed title-bar band (the visible band is the CSS
/// `.titlebar-strip`). Panel preview webviews (`url-preview-*`) are skipped.
/// Best-effort per window: a failure on one is logged, not fatal.
fn paint_title_bars(app: &tauri::AppHandle, color: tauri::utils::config::Color) {
    for (label, window) in app.windows() {
        if is_app_window(&label) {
            if let Err(e) = window.set_background_color(Some(color)) {
                eprintln!("[Tauri] Failed to set title-bar color on {label}: {e}");
            }
        }
    }
}

/// Frontend-driven window-background tint. The app's `applyTheme` calls this with
/// the header-top blue for the active theme (`#1a6fd0` light / `#15549e` dark) so
/// the behind-the-webview fallback for the reclaimed title-bar band tracks the
/// in-app header across theme switches. `color` is a CSS hex string (`#rgb` /
/// `#rrggbb` / `#rrggbbaa`). See `paint_title_bars` for why only the window layer
/// is colored.
#[tauri::command]
fn set_titlebar_color(app: tauri::AppHandle, color: String) -> Result<(), String> {
    let parsed = color
        .parse()
        .map_err(|e| format!("invalid color {color:?}: {e}"))?;
    paint_title_bars(&app, parsed);
    Ok(())
}

/// Start a native window drag for the calling window. The frontend's
/// `useWindowDragRegion` hook calls this once the pointer crosses a small
/// movement threshold while pressed on a non-interactive area of the title-bar
/// band, so plain clicks / double-clicks still reach the page's own handlers (the
/// header's double-click → pane-maximize, etc.). This replaces
/// `data-tauri-drag-region`, whose internal `plugin:window|start_dragging` IPC is
/// denied by our capability ACL — only app-defined commands like this one bypass
/// the ACL. `window` is the calling window, so New-Window windows drag themselves.
#[tauri::command]
fn start_window_drag(window: tauri::Window) -> Result<(), String> {
    window.start_dragging().map_err(|e| format!("{e}"))
}

/// Toggle the calling window between maximized (macOS zoom) and restored. Bound to
/// a double-click on the reclaimed title-bar strip only — the header keeps its own
/// double-click → pane-maximize. Like `start_window_drag`, an app command so it
/// isn't subject to the window-plugin ACL.
#[tauri::command]
fn toggle_window_maximize(window: tauri::Window) -> Result<(), String> {
    if window.is_maximized().map_err(|e| format!("{e}"))? {
        window.unmaximize().map_err(|e| format!("{e}"))
    } else {
        window.maximize().map_err(|e| format!("{e}"))
    }
}

/// Open an additional top-level app window (File → New Window / Cmd+N). Every
/// window is just another client of the same engine — the engine + Postgres run
/// as a shared launchd service (see `desktop`), so all windows share one
/// workspace stack. The WKWebView crash-recovery watchdog stays scoped to
/// `main`.
fn open_new_window(app: &tauri::AppHandle) -> Result<(), String> {
    let counter = WEBVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("{APP_WINDOW_PREFIX}{counter}");

    // `titleBarStyle: "Overlay"` + hidden title is set per-window: the
    // tauri.conf.json values only apply to the declared `main` window, so
    // builder-made windows need them explicitly or they'd render the default
    // opaque (black) bar. The builder methods are macOS-only, so they're applied
    // via a cfg-gated shadow (the rest of this crate stays cross-platform
    // compilable).
    let builder = WebviewWindowBuilder::new(app, &label, new_window_url(app))
        .title("Lucidos")
        .inner_size(1024.0, 768.0);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    builder.build().map_err(|e| format!("{e}"))?;
    // Tint the bar immediately (window layer only) so it's not black for the
    // moment before this window's frontend boots and calls `set_titlebar_color`.
    // The window is registered by `build()`, so paint_title_bars now covers it.
    if let Ok(color) = TITLE_BAR_DEFAULT_COLOR.parse() {
        paint_title_bars(app, color);
    }
    Ok(())
}

/// The URL a freshly opened app window should load. Mirrors the main window's
/// current URL when it has already navigated to the gateway (so the new window
/// lands on the same workspace/route the user is viewing); falls back to the
/// gateway on the stable packaged port, or the bundled entry
/// in dev (the dev server is wired via `tauri.conf.json` `devUrl`).
fn new_window_url(app: &tauri::AppHandle) -> WebviewUrl {
    if let Some(url) = app.get_webview_window("main").and_then(|w| w.url().ok()) {
        if url.scheme() == "http" || url.scheme() == "https" {
            return WebviewUrl::External(url);
        }
    }
    if !tauri::is_dev() {
        // Same builder `desktop::launch` navigates the main window with, so a New
        // Window opened before that navigation still lands on the exact origin the
        // gateway ACL capability was pinned to (desktop::gateway_capability).
        if let Ok(url) = desktop::gateway_url(desktop::engine_port()).parse::<tauri::Url>() {
            return WebviewUrl::External(url);
        }
    }
    WebviewUrl::App("index.html".into())
}

/// Compute title bar gap: difference between Tauri's window logical height
/// and the CSS viewport height (window.innerHeight from frontend).
fn title_bar_gap(app: &tauri::AppHandle, viewport_height: f64) -> f64 {
    get_main_window(app)
        .map(|window| {
            let scale = window.scale_factor().unwrap_or(1.0);
            let window_h = window
                .inner_size()
                .map(|s| s.height as f64 / scale)
                .unwrap_or(0.0);
            (window_h - viewport_height).max(0.0)
        })
        .unwrap_or(0.0)
}

/// Helper: get the active panel webview, if any.
fn get_panel_webview(app: &tauri::AppHandle) -> Option<tauri::Webview> {
    let state = app.state::<PanelWebview>();
    let label = state.0.lock().unwrap().clone()?;
    app.get_webview(&label)
}

fn close_existing(app: &tauri::AppHandle, state: &PanelWebview) {
    let label = state.0.lock().unwrap().take();
    if let Some(label) = label {
        if let Some(wv) = app.get_webview(&label) {
            let _ = wv.eval("if(window.__lucidos_title_cleanup) window.__lucidos_title_cleanup()");
            let _ = wv.close();
        }
    }
}

#[tauri::command]
fn create_panel_webview(
    app: tauri::AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    viewport_height: Option<f64>,
) -> Result<String, String> {
    let state = app.state::<PanelWebview>();
    close_existing(&app, &state);

    let parsed_url: tauri::Url = url.parse().map_err(|e| format!("{e}"))?;
    let counter = WEBVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("url-preview-{counter}");

    let window = get_main_window(&app).ok_or("main window not found")?;
    let gap = viewport_height
        .map(|vh| title_bar_gap(&app, vh))
        .unwrap_or(0.0);

    let page_load_app = app.clone();
    let builder = tauri::webview::WebviewBuilder::new(&label, WebviewUrl::External(parsed_url))
        .user_agent(safari_user_agent())
        .on_navigation(|_nav_url| true)
        .on_new_window(move |url, _features| {
            open_in_default_browser(url.as_str());
            tauri::webview::NewWindowResponse::Deny
        })
        .on_page_load(move |wv, payload| {
            // on_page_load fires only for MAIN FRAME navigations (backed by
            // WKWebView's didCommitNavigation/didFinishNavigation on macOS).
            match payload.event() {
                PageLoadEvent::Started => {
                    // Grab title early from <head> — reduces visible delay
                    if let Err(e) = wv.eval(TITLE_OBSERVER_JS) {
                        eprintln!("[Tauri] Failed to inject title observer: {e}");
                    }
                }
                PageLoadEvent::Finished => {
                    let url = payload.url().to_string();
                    let _ = page_load_app.emit_to("main", "panel-url-changed", url);
                    // Re-inject to catch final title + set up observer for SPA changes
                    if let Err(e) = wv.eval(TITLE_OBSERVER_JS) {
                        eprintln!("[Tauri] Failed to inject title observer: {e}");
                    }
                    if let Err(e) = wv.eval(URL_OBSERVER_JS) {
                        eprintln!("[Tauri] Failed to inject URL observer: {e}");
                    }
                }
            }
        });

    let position = tauri::Position::Logical(tauri::LogicalPosition::new(x, y + gap));
    let size = tauri::Size::Logical(tauri::LogicalSize::new(width, height + gap));

    window
        .add_child(builder, position, size)
        .map_err(|e| format!("{e}"))?;

    *state.0.lock().unwrap() = Some(label.clone());

    Ok(label)
}

#[tauri::command]
fn navigate_panel_webview(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let state = app.state::<PanelWebview>();
    let label = state.0.lock().unwrap().clone();
    let label = label.ok_or("panel webview not found")?;
    let wv = app.get_webview(&label).ok_or("panel webview not found")?;

    let parsed_url: tauri::Url = url.parse().map_err(|e| format!("{e}"))?;
    wv.navigate(parsed_url).map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
fn close_panel_webview(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<PanelWebview>();
    close_existing(&app, &state);
    Ok(())
}

#[tauri::command]
fn set_panel_webview_bounds(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    viewport_height: Option<f64>,
) -> Result<(), String> {
    let gap = viewport_height
        .map(|vh| title_bar_gap(&app, vh))
        .unwrap_or(0.0);
    let state = app.state::<PanelWebview>();
    let label = state.0.lock().unwrap().clone();
    if let Some(label) = label {
        if let Some(wv) = app.get_webview(&label) {
            wv.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(
                x,
                y + gap,
            )))
            .map_err(|e| format!("{e}"))?;
            wv.set_size(tauri::Size::Logical(tauri::LogicalSize::new(
                width,
                height + gap,
            )))
            .map_err(|e| format!("{e}"))?;
        }
    }
    Ok(())
}

#[tauri::command]
fn hide_panel_webview(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(wv) = get_panel_webview(&app) {
        wv.hide().map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

#[tauri::command]
fn show_panel_webview(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(wv) = get_panel_webview(&app) {
        wv.show().map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

#[tauri::command]
fn webview_go_back(app: tauri::AppHandle) -> Result<(), String> {
    let wv = get_panel_webview(&app).ok_or("panel webview not found")?;
    wv.eval("window.history.back()").map_err(|e| e.to_string())
}

#[tauri::command]
fn webview_go_forward(app: tauri::AppHandle) -> Result<(), String> {
    let wv = get_panel_webview(&app).ok_or("panel webview not found")?;
    wv.eval("window.history.forward()")
        .map_err(|e| e.to_string())
}

/// JS injected on page load: reports title immediately, then observes for SPA title changes.
/// No polling — uses MutationObserver on <title> (and on <head> to detect late-appearing <title>).
const TITLE_OBSERVER_JS: &str = r#"(function(){
    if(window.__lucidos_title_cleanup) window.__lucidos_title_cleanup();
    var lastTitle='',titleObserver,headObserver;
    function reportTitle(){
        var title=document.title||'';
        if(title!==lastTitle){lastTitle=title;window.__TAURI_INTERNALS__&&window.__TAURI_INTERNALS__.invoke('__panel_title_report',{title:title});}
    }
    function watchTitle(){
        var el=document.querySelector('title');
        if(el){
            if(headObserver){headObserver.disconnect();headObserver=null;}
            titleObserver=new MutationObserver(reportTitle);
            titleObserver.observe(el,{childList:true,characterData:true,subtree:true});
        }
    }
    reportTitle();
    watchTitle();
    if(!titleObserver&&document.head){
        headObserver=new MutationObserver(function(){if(document.querySelector('title'))watchTitle();});
        headObserver.observe(document.head,{childList:true});
    }
    window.__lucidos_title_cleanup=function(){
        if(titleObserver)titleObserver.disconnect();if(headObserver)headObserver.disconnect();
    };
})()"#;

/// JS injected on page load: reports URL changes from back/forward navigation (popstate)
/// and SPA client-side routing (pushState/replaceState). These navigations don't trigger
/// WKWebView's on_page_load, so without this the frontend's panelUrl gets out of sync.
const URL_OBSERVER_JS: &str = r#"(function(){
    if(window.__lucidos_url_cleanup) window.__lucidos_url_cleanup();
    var T=window.__TAURI_INTERNALS__;
    if(!T) return;
    var lastUrl='';
    function reportUrl(){
        var url=location.href;
        if(url!==lastUrl){lastUrl=url;T.invoke('__panel_url_report',{url:url});}
    }
    function onPageShow(e){if(e.persisted){lastUrl='';reportUrl();}}
    window.addEventListener('popstate',reportUrl);
    window.addEventListener('pageshow',onPageShow);
    var origPush=history.pushState,origReplace=history.replaceState;
    history.pushState=function(){origPush.apply(this,arguments);reportUrl();};
    history.replaceState=function(){origReplace.apply(this,arguments);reportUrl();};
    window.__lucidos_url_cleanup=function(){
        window.removeEventListener('popstate',reportUrl);
        window.removeEventListener('pageshow',onPageShow);
        history.pushState=origPush;history.replaceState=origReplace;
    };
})()"#;

#[tauri::command]
fn __panel_title_report(app: tauri::AppHandle, title: String) -> Result<(), String> {
    let _ = app.emit_to("main", "panel-title-changed", title);
    Ok(())
}

#[tauri::command]
fn __panel_url_report(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let _ = app.emit_to("main", "panel-url-changed", url);
    Ok(())
}

/// Extract the text content and title from the panel webview.
/// Uses a sync channel: eval JS → JS calls __panel_content_report → channel resolves.
/// Runs on a blocking thread so it doesn't block the main thread while waiting.
#[tauri::command]
async fn webview_get_content(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let wv = get_panel_webview(&app).ok_or("panel webview not found")?;

    let (tx, rx) = std::sync::mpsc::channel();
    {
        let state = app.state::<PanelContentChannel>();
        *state.0.lock().unwrap() = Some(tx);
    }

    // Extract text content from the page body, truncated to 100K chars
    wv.eval(
        r#"(function(){
            var title = document.title || '';
            var content = (document.body && document.body.innerText) || '';
            if (content.length > 100000) content = content.substring(0, 100000);
            window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke(
                '__panel_content_report',
                { title: title, content: content }
            );
        })()"#,
    )
    .map_err(|e| e.to_string())?;

    // Wait up to 5 seconds for the JS callback (blocking receive with timeout)
    let (title, content) = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "content extraction timed out".to_string())?;

    Ok(serde_json::json!({ "title": title, "content": content }))
}

/// Restart the GUI **client** (relaunch the window shell). This does NOT touch
/// the always-on gateway service — that runs as a launchd LaunchAgent,
/// independent of the window (see `desktop`). To restart the service itself,
/// use `restart_service` (packaged) or `/api/v1/restart` (dev workspace stack).
///
/// A packaged build relaunches through LaunchServices so the new instance comes
/// back FRONTMOST ([`desktop::schedule_relaunch_after_exit`] explains why a
/// direct respawn does not). Every other case (dev, an unbundled binary, a
/// watcher we could not spawn) falls back to replacing this process with a fresh
/// one, which is what this command always did.
///
/// Sync command, so it runs on the main thread: both `save_window_state` and
/// `cleanup_before_exit` require that.
#[tauri::command]
fn restart_app(app: tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Failed to get current exe: {e}"))?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();

    eprintln!("[Tauri] Restarting app: {:?} {:?}", exe, args);

    // Persist geometry before we go. The plugin's exit-time flush does not run on
    // the exec path at all, so without this an in-session move/resize would be
    // lost across the restart.
    if let Err(e) = app.save_window_state(window_state_flags()) {
        eprintln!("[Tauri] Failed to persist window state before restart: {e}");
    }

    match desktop::schedule_relaunch_after_exit() {
        Ok(()) => {
            // The watcher is armed and only fires once we are gone, so this exit
            // IS the relaunch. Never fall through to the respawn below, or the
            // watcher's `open` would find a live client and we would end up with
            // two.
            app.cleanup_before_exit();
            std::process::exit(0);
        }
        Err(e) => eprintln!("[Tauri] No LaunchServices relaunch ({e}); respawning directly"),
    }

    app.cleanup_before_exit();

    // On Unix, exec() replaces the process in-place. On other platforms, spawn + exit.
    restart_process(&exe, &args)
}

/// Save the window geometry, then exit the client, from the MAIN thread, and
/// never come back.
///
/// For a caller on the async runtime that has already arranged its own relaunch
/// ([`desktop::schedule_relaunch_after_exit`]). Both halves need the main thread:
/// `save_window_state` deadlocks off it (see [`persist_window_state_on_main`]),
/// and `cleanup_before_exit` tears down webviews.
///
/// The save is explicit because `cleanup_before_exit` does NOT dispatch
/// `RunEvent::Exit`, which is what normally drives the window-state plugin's own
/// exit-time write. `app.restart()` went out through the real exit event and got
/// that write for free; this path has to do it itself or a move/resize inside the
/// debounce window would be lost across the relaunch. It runs inline rather than
/// through [`persist_window_state_on_main`] because that helper posts its own
/// main-thread task, which would never be reached: this closure exits first.
///
/// Parks instead of returning: the caller's next move would be its fallback
/// respawn, and that would launch a second client alongside the one the watcher
/// is about to bring up.
pub(crate) fn exit_after_relaunch_scheduled(app: &tauri::AppHandle) -> ! {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Err(e) = handle.save_window_state(window_state_flags()) {
            eprintln!("[Tauri] Failed to persist window state before relaunch: {e}");
        }
        handle.cleanup_before_exit();
        std::process::exit(0);
    }) {
        // The event loop is unreachable, so nothing will run the clean exit for
        // us. Go without it: geometry is already on disk as of the last debounced
        // flush, and a client that refuses to die would leave the watcher
        // activating the version we just replaced.
        eprintln!("[Tauri] Could not marshal the exit onto the main thread: {e}");
        std::process::exit(0);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// Open a URL in the system default browser (not the embedded webview). Used by
/// the Mobile Access page for the Tailscale download link.
#[tauri::command]
fn open_url_external(url: String) {
    open_in_default_browser(&url);
}

/// Restart the always-on gateway service via launchd (`launchctl kickstart -k`).
/// The packaged "Restart" control routes here (the dev `/api/v1/restart` script
/// isn't in the bundle). The supervisor catches the SIGTERM, tears the bundled
/// gateway and its spawned workspace engines down gracefully, and launchd
/// respawns the service.
#[tauri::command]
fn restart_service() -> Result<(), String> {
    desktop::restart_service()
}

/// Full teardown: confirm with the user, then stop the always-on gateway service
/// (`launchctl bootout`) and exit the client. This is the ONLY path that stops
/// the service — closing the window / Cmd+Q merely hide the client (it stays
/// resident in the menu bar), so triggers, scheduled tasks, coding-agent
/// sessions, and push keep running. Reached from the menu-bar "Quit and Stop
/// Background Service" item and the app-menu item of the same name.
///
/// Because stopping the service silently halts all of that background work until
/// the next launch, a native confirm spells out the consequence and points at
/// the non-destructive alternative (close the window). Only on confirm does it
/// set `QUITTING` (so the `ExitRequested` guard lets this exit through), stop the
/// service, and exit. The next app launch re-installs and re-bootstraps the
/// service.
#[tauri::command]
fn quit_lucidos(app: tauri::AppHandle) {
    let quit_app = app.clone();
    app.dialog()
        .message(
            "Stopping the background service means your triggers and scheduled tasks won't \
             run, coding-agent sessions will stop, and you won't receive notifications — \
             until you open Lucidos again.\n\nTo keep them running, close the window instead. \
             Lucidos stays in the menu bar.",
        )
        .title("Quit and Stop Background Service")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Quit and Stop Service".to_string(),
            "Cancel".to_string(),
        ))
        .show(move |proceed| {
            if !proceed {
                return; // Cancel / Escape — leave the client and service running.
            }
            QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
            desktop::stop_service();
            quit_app.exit(0);
        });
}

/// Fully uninstall Lucidos from the GUI — modeled on Docker Desktop's uninstall
/// so a non-developer never needs a terminal. A two-step native confirm (the
/// `tauri_plugin_dialog` plugin caps at two buttons, so the keep-vs-delete choice
/// is its own dialog): first confirm the uninstall, then choose whether to also
/// delete all data. The plugin can't make a non-default button the native
/// default, so the affirmative button is highlighted; the scary copy + explicit
/// two-button choice + result dialog provide the safety. On success the result
/// dialog dismisses into a hard process exit (NOT `app.exit`, which would run
/// Tauri's on-exit handlers and let the window-state plugin re-create the data
/// dir we just deleted); on error the app keeps running so the user can retry.
#[tauri::command]
fn uninstall_lucidos(app: tauri::AppHandle) {
    let confirm_app = app.clone();
    app.dialog()
        .message(
            "This will quit Lucidos, stop the background service, and move the Lucidos app to \
             the Trash.",
        )
        .title("Uninstall Lucidos")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Continue".to_string(),
            "Cancel".to_string(),
        ))
        .show(move |proceed| {
            if !proceed {
                return; // Cancel / Escape — no-op.
            }
            // Step 2: data fate. The cancel-slot ("Keep My Data", false) is the
            // SAFE choice, so Escape here keeps data and still proceeds with the
            // uninstall (the user already committed by clicking Continue).
            let fate_app = confirm_app.clone();
            confirm_app
                .dialog()
                .message(
                    "Do you also want to permanently delete all Lucidos data, including your \
                     local database and all workspaces? This cannot be undone.\n\nChoose Keep My \
                     Data to preserve it so you can reinstall and pick up where you left off.",
                )
                .title("Uninstall Lucidos")
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "Delete Everything".to_string(),
                    "Keep My Data".to_string(),
                ))
                .show(move |delete_data| {
                    // The uninstall is committed (both data-fate buttons proceed —
                    // only the data deletion differs). Hide the client window(s)
                    // NOW, before the destructive steps tear down the engine +
                    // service: otherwise the user stares at a window whose backend
                    // is dying and the frontend spews "engine unreachable" errors.
                    // The result/error dialogs are app-level, so they still show
                    // with no window visible; on failure `run_uninstall` re-shows
                    // the main window so the user can retry.
                    hide_all_windows(&fate_app);
                    run_uninstall(fate_app.clone(), delete_data);
                });
        });
}

/// Hide every Lucidos client window (the declared `main` plus any New-Window
/// children). Called the instant an uninstall is confirmed so the about-to-be-torn
/// -down backend can't surface a cascade of frontend errors behind the result
/// dialog. Best-effort: a hide failure must not abort the uninstall. Window
/// messages are proxied to the main event loop, so this is safe from the dialog
/// callback thread.
fn hide_all_windows(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        let _ = window.hide();
    }
}

/// Resolve the data dir, run the destructive uninstall off the dialog-callback
/// thread (so the UI doesn't freeze during `pg_ctl stop` / Finder), then report
/// the outcome natively. On success, exit; on error, keep running.
fn run_uninstall(app: tauri::AppHandle, delete_data: bool) {
    std::thread::spawn(move || {
        let app_data = match desktop::app_data_dir_from_env() {
            Ok(p) => p,
            Err(e) => {
                report_uninstall_error(
                    &app,
                    &format!("Could not resolve the Lucidos data directory: {e}"),
                );
                return;
            }
        };
        match desktop::uninstall(&app_data, delete_data) {
            Ok(()) => {
                let body = if delete_data {
                    "Lucidos has been uninstalled and all its data deleted. The app has been \
                     moved to the Trash."
                        .to_string()
                } else {
                    format!(
                        "Lucidos has been uninstalled. Your data was preserved at {}. The app has \
                         been moved to the Trash.",
                        app_data.display()
                    )
                };
                app.dialog()
                    .message(body)
                    .title("Uninstall Lucidos")
                    .buttons(MessageDialogButtons::Ok)
                    // Hard-exit rather than app.exit(0): a normal exit runs
                    // Tauri's RunEvent::Exit handlers, and the window-state
                    // plugin would re-persist `.window-state.json` into a freshly
                    // re-created `~/Library/Application Support/com.lucidos.app`,
                    // leaving residue after a full "Delete Everything".
                    .show(|_| std::process::exit(0));
            }
            Err(e) => report_uninstall_error(&app, &e),
        }
    });
}

/// Surface an uninstall failure in a native dialog (and the logs). Does NOT
/// exit — the user can retry. Re-shows the main window, which was hidden when the
/// uninstall was confirmed, so the user isn't stranded with no window to retry
/// from.
fn report_uninstall_error(app: &tauri::AppHandle, msg: &str) {
    eprintln!("[Tauri] Uninstall failed: {msg}");
    show_main_window(app);
    app.dialog()
        .message(format!("Uninstall did not complete:\n\n{msg}"))
        .title("Uninstall Lucidos")
        .buttons(MessageDialogButtons::Ok)
        .show(|_| {});
}

#[cfg(unix)]
fn restart_process(exe: &std::path::Path, args: &[std::ffi::OsString]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(exe).args(args).exec();
    Err(format!("exec failed: {err}"))
}

#[cfg(not(unix))]
fn restart_process(exe: &std::path::Path, args: &[std::ffi::OsString]) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(args)
        .spawn()
        .map_err(|e| format!("Failed to spawn: {e}"))?;
    std::process::exit(0);
}

#[tauri::command]
fn heartbeat(app: tauri::AppHandle) {
    let state = app.state::<LastHeartbeat>();
    *state.at.lock().unwrap() = Instant::now();
    state
        .count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Bring the main window to the front.
///
/// The same reshow the tray's "Open Lucidos" and a notification tap perform
/// ([`show_main_window`]: leave menu-bar-only, unminimize, show, focus, activate
/// frontmost), exposed to the page for the one flow that finishes somewhere
/// else: an OAuth authorization the user completes in a browser. Without it they
/// approve the provider's consent screen and are left staring at the callback
/// tab with Lucidos behind it.
///
/// Deliberately NOT a general "focus me" the page may call whenever it likes.
/// Its one caller (`handleOAuthAccountConnected`) fires only for a flow THIS
/// device started, seconds after the user's own click, and only once. A window
/// that fronts itself on a background event is a nuisance, so keep new callers
/// to that shape.
#[tauri::command]
fn focus_main_window(app: tauri::AppHandle) {
    show_main_window(&app);
}

#[tauri::command]
fn __panel_content_report(
    app: tauri::AppHandle,
    title: String,
    content: String,
) -> Result<(), String> {
    let state = app.state::<PanelContentChannel>();
    if let Some(tx) = state.0.lock().unwrap().take() {
        let _ = tx.send((title, content));
    }
    Ok(())
}

/// Show a native macOS notification and route a tap to the page.
///
/// Delegates to the [`notifications`] module, which drives Apple's modern
/// `UserNotifications` framework (`UNUserNotificationCenter`). `link` is the
/// SW-message shape (`notification_id` / `thread_id` / `event_id` / `tap`); on
/// tap the delegate emits `native-notification-tapped` carrying it, and the
/// page routes it through the SAME `dispatchDeepLink` the web-push tap uses (see
/// `store/actions/native-push.ts`). No-op in `tauri dev` (unbundled binary) and
/// on non-macOS — see `notifications.rs` and `system-knowhow/notifications.md`
/// §4.
#[tauri::command]
fn show_native_notification(title: String, body: String, link: serde_json::Value) {
    notifications::show(&title, &body, link);
}

/// Remove an already-delivered native banner — the cross-device dismiss
/// counterpart of [`show_native_notification`]. `id = Some(notification_id)`
/// removes that one banner; `None` removes ALL delivered banners (the
/// mark-all-read path). Driven by the engine's `NativePushDismissRequested` SSE
/// when a notification is read on any device (`store/actions/native-push.ts`).
/// Delegates to the [`notifications`] module, which also drops the stashed deep
/// link so a tap on a now-removed banner can't route. No-op in `tauri dev`
/// (unbundled binary) and on non-macOS — see `notifications.rs`.
#[tauri::command]
fn dismiss_native_notification(id: Option<String>) {
    notifications::dismiss(id);
}

/// Wake the dock-badge loop for an immediate recompute. The frontend calls this
/// (under `isTauri()`) from its notification SSE handler so the macOS dock badge
/// updates the instant a notification is read — in-app or from another device —
/// instead of waiting for the periodic poll. Best-effort: a send failure (no
/// receiver in dev / non-macOS, where there is no dock tile) is ignored. The
/// recompute itself reads the gateway's fresh `unread-total` aggregate. See
/// `desktop::launch` and `system-knowhow/notifications.md` § App-icon badge.
#[tauri::command]
fn nudge_dock_badge(state: tauri::State<'_, DockBadgeNudge>) {
    let _ = state.0.lock().unwrap().send(());
}

/// Report whether the main app window is currently *active* — focused AND
/// on-screen (visible, not minimized). The frontend pulls this at startup to
/// SEED its `native-window-active` cache BEFORE registering the event listener
/// (`utils/nativeWindow.ts`): Tauri does not replay the transition events
/// emitted from `on_window_event` to a listener that registers after the fact,
/// and the cache defaults to `true`. Without the seed, a freshly (re)loaded
/// page that is actually backgrounded/trayed — crash-watchdog reload, version
/// refresh, cold/unfocused launch — keeps the `true` default and wrongly pongs
/// the device as active, so the engine suppresses the OS push into an invisible
/// in-app toast (no banner). See `system-knowhow/notifications.md` §3/§4.
///
/// Any state read that fails resolves to the SAFE direction (inactive →
/// `false`), so an uncertain seed surfaces the banner rather than suppressing
/// it. No-op-ish off the main window (returns `false`).
#[tauri::command]
fn get_native_window_active(app: tauri::AppHandle) -> bool {
    get_main_window(&app)
        .map(|w| {
            let focused = w.is_focused().unwrap_or(false);
            // Conservative on read failure: treat unknown visibility as not
            // visible so we never wrongly report active (which would suppress).
            let visible = w.is_visible().unwrap_or(false);
            let minimized = w.is_minimized().unwrap_or(false);
            focused && visible && !minimized
        })
        .unwrap_or(false)
}

/// Drain the deep links from native-banner taps the page may not have been
/// listening for (see `notifications::take_pending_taps`). The frontend calls
/// this at startup (cold path) and on each `native-notification-tapped` signal
/// (warm path); the drain is atomic, so each tap routes exactly once. Naturally
/// empty in `tauri dev` / off-macOS. See `store/actions/native-push.ts`.
#[tauri::command]
fn take_pending_native_taps() -> Vec<serde_json::Value> {
    notifications::take_pending_taps()
}

/// Bridge the native window's *active* state (focused AND on-screen) to that
/// window's webview as a `native-window-active` event (a bare bool). The embedded
/// WKWebView can't observe macOS `orderOut:` — a window dismissed to the menu-bar
/// tray keeps `document.visibilityState='visible'` and `document.hasFocus()=true`
/// — and its `hasFocus()` is unreliable generally, so the page can't tell "in
/// use" from "trayed / behind another app" on its own. The frontend caches this
/// and feeds it into `isPageActive()` so a non-active desktop client gets the OS
/// native banner instead of a suppressed, invisible in-app toast. Targeted to the
/// specific window so a secondary New-Window client keeps its own state. See
/// `utils/nativeWindow.ts` and `system-knowhow/notifications.md` §1, §4.
fn emit_window_active(app: &tauri::AppHandle, label: &str, active: bool) {
    let _ = app.emit_to(label, "native-window-active", active);
}

/// True while the packaged macOS client is running menu-bar-only: it has no
/// visible app window, so it uses the `Accessory` activation policy and is absent
/// from the Dock and the Cmd+Tab app switcher, living only in the menu-bar tray.
/// Flipped by [`set_menu_bar_only`]; read by [`apply_unread_indicator`] to pick
/// which surface shows the unread count. Starts `false` — the config declares a
/// visible `main` window, so the app launches as a normal `Regular` Dock app.
static MENU_BAR_ONLY: AtomicBool = AtomicBool::new(false);

/// The most recent aggregate unread total, so a `Regular`↔`Accessory` transition
/// can re-route the SAME count to the surface that now exists (Dock badge vs tray
/// title) without waiting for the next badge-loop poll. Written by
/// [`apply_unread_indicator`].
static LAST_UNREAD: AtomicU64 = AtomicU64::new(0);

/// Pure: the client should be menu-bar-only exactly when no app window is visible.
/// Split out so the rule is unit-testable without a running app.
fn should_be_menu_bar_only(visible_app_windows: usize) -> bool {
    visible_app_windows == 0
}

/// Pure: which surface shows the unread `count` for the current activation state,
/// as `(dock_badge_label, tray_title_label)`. `Regular` → the Dock badge; menu-bar
/// only → the tray-icon title (there is no Dock tile then). `0` clears both; a
/// count over 99 collapses to "99+". Unit-testable without AppKit.
fn unread_targets(menu_bar_only: bool, count: u64) -> (Option<String>, Option<String>) {
    if count == 0 {
        return (None, None);
    }
    let label = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    if menu_bar_only {
        (None, Some(label))
    } else {
        (Some(label), None)
    }
}

/// Route the aggregate unread `count` to the correct surface for the current
/// activation state: the Dock badge while a window is open (`Regular`), the
/// menu-bar tray-icon title while menu-bar-only (`Accessory`). Stores `count` in
/// [`LAST_UNREAD`] so a later activation transition can re-route it. Called from
/// the desktop badge loop (marshalled to the main thread) and from
/// [`set_menu_bar_only`].
pub(crate) fn apply_unread_indicator(app: &tauri::AppHandle, count: u64) {
    LAST_UNREAD.store(count, std::sync::atomic::Ordering::SeqCst);
    let menu_bar_only = MENU_BAR_ONLY.load(std::sync::atomic::Ordering::SeqCst);
    let (dock, tray) = unread_targets(menu_bar_only, count);
    notifications::set_dock_badge(dock);
    notifications::set_tray_title(app, tray);
}

/// Switch the client between menu-bar-only (`Accessory`: gone from the Dock +
/// Cmd+Tab) and normal (`Regular`: Dock + Cmd+Tab). On macOS this sets the
/// NSApplication activation policy; on other platforms only the flag moves (the
/// badge/tray writes are no-ops there). Then re-routes the unread indicator to the
/// surface that now exists. See
/// `docs/plans/2026-07-01-macos-client-menu-bar-only-on-window-close.md`.
fn set_menu_bar_only(app: &tauri::AppHandle, menu_bar_only: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if menu_bar_only {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        };
        if let Err(e) = app.set_activation_policy(policy) {
            eprintln!("[Tauri] Failed to set activation policy: {e}");
        }
    }
    MENU_BAR_ONLY.store(menu_bar_only, std::sync::atomic::Ordering::SeqCst);
    apply_unread_indicator(app, LAST_UNREAD.load(std::sync::atomic::Ordering::SeqCst));
}

/// Drop the client to menu-bar-only IFF no app window is left visible. Called after
/// a window is hidden (`CloseRequested` on `main`) or destroyed (a secondary
/// New-Window closing), so closing the LAST window removes the app from the Dock +
/// Cmd+Tab, while closing one of several leaves it a normal Dock app. `excluding`
/// skips a window that is on its way out but might still be listed.
fn enter_menu_bar_only_if_no_windows(app: &tauri::AppHandle, excluding: Option<&str>) {
    let visible = app
        .webview_windows()
        .iter()
        .filter(|(label, w)| {
            is_app_window(label.as_str())
                && Some(label.as_str()) != excluding
                && w.is_visible().unwrap_or(false)
        })
        .count();
    if should_be_menu_bar_only(visible) {
        set_menu_bar_only(app, true);
    }
}

/// Activate the app frontmost (macOS). No-op elsewhere. See [`show_main_window`].
fn activate_app_frontmost() {
    #[cfg(target_os = "macos")]
    notifications::activate_app();
}

/// Close the whole client to the menu-bar tray — the Cmd+Q / "Close to Menu Bar"
/// action. Destroys secondary New-Window windows and HIDES `main` (so reopen is
/// instant and preserves page state), then drops to menu-bar-only. The always-on
/// launchd services are untouched — the only full teardown is [`quit_lucidos`].
///
/// Packaged only. In dev there is no always-on service and no menu-bar tray
/// (`install_tray` is skipped in dev), so hiding + going `Accessory` would strand
/// the window with no way to reopen it and leave `tauri-dev.sh` running headless.
/// So in dev this closes the window(s) instead, matching the default close-quits
/// behavior (the last window closing lets the process exit) — the same reason
/// `CloseRequested`/`Destroyed` above are `!is_dev()`-guarded.
fn close_all_to_tray(app: &tauri::AppHandle) {
    if tauri::is_dev() {
        for (label, window) in app.webview_windows() {
            if is_app_window(&label) {
                let _ = window.close();
            }
        }
        return;
    }
    // Flush geometry now — the window-state plugin's exit-time write never runs
    // (we hide, never exit), so this is the moment to remember size/position.
    if let Err(e) = app.save_window_state(window_state_flags()) {
        eprintln!("[Tauri] Failed to persist window state on close-to-tray: {e}");
    }
    for (label, window) in app.webview_windows() {
        if label == "main" {
            let _ = window.hide();
            emit_window_active(app, "main", false);
        } else if is_app_window(&label) {
            let _ = window.close();
        }
    }
    enter_menu_bar_only_if_no_windows(app, None);
}

/// Show + focus the main window, recreating it if it was destroyed. Backs the
/// menu-bar "Open Lucidos" item, the macOS Dock-click (Reopen), and a
/// native-notification tap, so a window hidden on close can always be brought
/// back. Leaves menu-bar-only first (restores the `Regular` activation policy) so
/// the window can come forward with a clickable app menu, then emits
/// `native-window-active = true` so the reshown page immediately counts as active.
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    // Leaving menu-bar-only: restore `Regular` BEFORE showing so the window can be
    // fronted and the app menu is clickable (the AppKit `Accessory`→`Regular`
    // transition otherwise leaves the app behind / the menu bar unclickable).
    set_menu_bar_only(app, false);

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        // set_focus alone can leave an app that just left `Accessory` behind other
        // apps / with an unclickable menu bar — explicitly activate frontmost.
        activate_app_frontmost();
        // `set_focus()` also fires `WindowEvent::Focused(true)`, but emit
        // explicitly so the reshow is deterministic regardless of event timing.
        emit_window_active(app, "main", true);
    } else if let Err(e) = open_new_window(app) {
        eprintln!("[Tauri] Failed to open window: {e}");
    }
}

/// Install the macOS menu-bar (tray) status item — packaged builds only. It keeps
/// the client resident after the window is dismissed: "Open Lucidos" re-shows the
/// window and "Quit and Stop Background Service" is the only full teardown
/// (`quit_lucidos` → `bootout` + exit). The always-on launchd service is
/// unaffected by closing the window.
fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    // macOS menu-bar status items are *template images*: a transparent-background
    // silhouette the system renders monochrome and inverts for light/dark bars.
    // The full-colour app icon (`default_window_icon`) would otherwise sit in the
    // menu bar as a lone blue square among the system's monochrome items. We embed
    // a dedicated silhouette (the logo glyph from public/icons/icon-source.svg,
    // no background) and flag it `icon_as_template` so macOS uses only its alpha.
    // Asset: icons/tray-template.png — regenerate with icons/gen-tray-template.py.
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
    let open = MenuItem::with_id(app, "tray_open", "Open Lucidos", true, None::<&str>)?;
    let status = MenuItem::with_id(
        app,
        "tray_status",
        format!("Service running · localhost:{}", desktop::engine_port()),
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "tray_quit_stop",
        "Quit and Stop Background Service",
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &status,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    TrayIconBuilder::with_id("lucidos-tray")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Lucidos")
        .menu(&menu)
        .on_menu_event(|app, event| {
            if event.id() == "tray_open" {
                show_main_window(app);
            } else if event.id() == "tray_quit_stop" {
                quit_lucidos(app.clone());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Nudge channel for the dock-badge loop: the `nudge_dock_badge` command holds
    // the sender (managed state); the receiver is handed to `desktop::launch`,
    // whose macOS badge thread owns it. Created here so both ends are in scope.
    let (dock_badge_nudge_tx, dock_badge_nudge_rx) = std::sync::mpsc::channel::<()>();
    tauri::Builder::default()
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(window_state_flags())
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(PanelWebview(Mutex::new(None)))
        .manage(PanelContentChannel(Mutex::new(None)))
        .manage(LastHeartbeat {
            at: Mutex::new(Instant::now()),
            count: AtomicU64::new(0),
        })
        .manage(GeometrySaver {
            dirty: AtomicBool::new(false),
            last_change: Mutex::new(Instant::now()),
        })
        .manage(device_id_store::DeviceIdStore::default())
        .manage(updater::AppUpdateRun::default())
        .manage(mobile::MobileAccessRuns::default())
        .manage(DockBadgeNudge(Mutex::new(dock_badge_nudge_tx)))
        .invoke_handler(tauri::generate_handler![
            create_panel_webview,
            navigate_panel_webview,
            close_panel_webview,
            set_panel_webview_bounds,
            hide_panel_webview,
            show_panel_webview,
            webview_go_back,
            webview_go_forward,
            __panel_title_report,
            __panel_url_report,
            webview_get_content,
            __panel_content_report,
            heartbeat,
            restart_app,
            restart_service,
            quit_lucidos,
            uninstall_lucidos,
            open_url_external,
            show_native_notification,
            dismiss_native_notification,
            focus_main_window,
            nudge_dock_badge,
            get_native_window_active,
            take_pending_native_taps,
            updater::check_app_update,
            updater::install_app_update_and_restart,
            updater::cancel_app_update,
            set_titlebar_color,
            start_window_drag,
            toggle_window_maximize,
            mobile::get_connect_info,
            mobile::tailscale_up,
            mobile::tailscale_serve,
            mobile::cancel_tailscale_serve,
            device_id_store::get_or_create_device_id,
        ])
        .on_window_event(|window, event| {
            let app = window.app_handle();
            match event {
                // Packaged: closing a window must NOT quit the client or stop the
                // always-on service. Hide the main window instead of letting it
                // close — the client stays resident in the menu bar (only the tray
                // "Quit and Stop Background Service" tears down). Secondary windows
                // close normally; dev keeps the default close-quits behavior so
                // `tauri-dev.sh` returns when the window is closed.
                //
                // Flush window geometry synchronously here regardless of platform:
                // the plugin's own disk write (RunEvent::Exit) is unreachable in the
                // packaged client (we hide instead of exit), so this close is the
                // moment the user expects their size/position remembered.
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    if let Err(e) = app.save_window_state(window_state_flags()) {
                        eprintln!("[Tauri] Failed to persist window state on close: {e}");
                    }
                    if !tauri::is_dev() && window.label() == "main" {
                        // Red-X / Cmd+W on the main window: keep the client resident
                        // — HIDE it (fast reopen, page state preserved) instead of
                        // closing, then drop to the menu-bar tray if this was the
                        // last visible window (out of the Dock + Cmd+Tab). Secondary
                        // New-Window windows close normally (handled by Destroyed).
                        api.prevent_close();
                        let _ = window.hide();
                        // Trayed via orderOut: — the WKWebView won't report this
                        // (visibilityState stays 'visible', hasFocus() stays true)
                        // and `Focused(false)` may not fire on orderOut, so this
                        // is the load-bearing signal that the page is no longer
                        // active. Lets the engine send the OS native banner instead
                        // of a suppressed, invisible in-app toast.
                        emit_window_active(app, window.label(), false);
                        enter_menu_bar_only_if_no_windows(app, Some(window.label()));
                    }
                }
                // A secondary New-Window closed (red-X / Cmd+W). Re-evaluate: if that
                // was the last visible window, drop the client to the menu-bar tray.
                // Packaged only — dev has no menu-bar residency. `main` never reaches
                // here (its close is prevented + hidden above).
                tauri::WindowEvent::Destroyed
                    if !tauri::is_dev() && is_app_window(window.label()) =>
                {
                    enter_menu_bar_only_if_no_windows(app, Some(window.label()));
                }
                // Native focus changed (app switch, behind another app, Space
                // change, app-hide, minimize). Bridge it so isPageActive() honors
                // the desktop "active = visible AND focused" rule using the
                // authoritative AppKit state instead of the flaky WKWebView
                // hasFocus(). The trayed (orderOut:) case is covered by the
                // explicit emit in CloseRequested above. See emit_window_active.
                tauri::WindowEvent::Focused(focused) if is_app_window(window.label()) => {
                    emit_window_active(app, window.label(), *focused);
                }
                // Geometry changed — arm the debounced background flush. The plugin
                // keeps its own in-memory cache up to date from these same events;
                // we additionally schedule a disk write so the new geometry survives
                // a relaunch even though RunEvent::Exit never fires.
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
                    if is_app_window(window.label()) =>
                {
                    let saver = app.state::<GeometrySaver>();
                    *saver.last_change.lock().unwrap() = Instant::now();
                    saver
                        .dirty
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                _ => {}
            }
        })
        .on_menu_event(|app, event| {
            // "Quit and Stop Background Service" (quit_lucidos) is the only teardown;
            // "Close to Menu Bar" (Cmd+Q) closes every client window and drops to the
            // menu-bar tray (services keep running); "New Window" opens another client
            // window. Per-window close (red X / Cmd+W) is the standard Close Window
            // item, handled in on_window_event above, not here.
            if event.id() == "quit_lucidos" {
                quit_lucidos(app.clone());
            } else if event.id() == "close_to_menu_bar" {
                close_all_to_tray(app);
            } else if event.id() == "uninstall_lucidos" {
                uninstall_lucidos(app.clone());
            } else if event.id() == "new_window" {
                if let Err(e) = open_new_window(app) {
                    eprintln!("[Tauri] Failed to open new window: {e}");
                }
            }
        })
        .on_page_load(|webview, payload| {
            if is_app_window(webview.label()) && matches!(payload.event(), PageLoadEvent::Started) {
                let version = env!("LUCIDOS_APP_VERSION");
                // App version + (macOS) the title-bar inset, in one early eval so
                // the reclaimed-band CSS var is set before first paint.
                let script = format!(
                    "window.__LUCIDOS_APP_VERSION__ = '{version}';{}",
                    titlebar_inset_script()
                );
                if let Err(e) = webview.eval(script) {
                    eprintln!("[Tauri] Failed to inject startup script: {e}");
                }
            }
        })
        .setup(move |app| {
            // Install the app menu. The standard edit/window items keep the
            // usual shortcuts; Cmd+Q maps to "Close to Menu Bar" (closes all client
            // windows to the menu-bar tray) and the explicit "Quit and Stop
            // Background Service" item drives on_menu_event → quit_lucidos.
            // Best-effort: a menu build failure must not block app startup.
            if let Err(e) = install_app_menu(app) {
                eprintln!("[Tauri] Failed to install app menu: {e}");
            }

            // Tint the window background to match the in-app header (window layer
            // only — no webview/page-bg flash). Under Overlay this is the
            // behind-the-webview fallback for the reclaimed title-bar band. This is
            // the default dark-theme blue; the frontend's applyTheme refines it to
            // the exact per-theme color via set_titlebar_color once it knows the
            // theme.
            if let Ok(color) = TITLE_BAR_DEFAULT_COLOR.parse() {
                paint_title_bars(app.handle(), color);
            }

            // Packaged: a menu-bar status item keeps the client resident after the
            // window is dismissed and hosts the only full-teardown action. No-op
            // in dev (there is no always-on service to represent).
            if !tauri::is_dev() {
                if let Err(e) = install_tray(app) {
                    eprintln!("[Tauri] Failed to install tray icon: {e}");
                }
            }

            // WKWebView crash recovery watchdog: if the JS heartbeat stops
            // arriving, the content process likely crashed (white screen), so
            // reload the webview to recover. A reload that brings back no
            // heartbeat at all did not fix anything, so the interval backs off
            // (see ReloadWatchdog) instead of thrashing once a minute forever —
            // and says so, because a silent reload loop is what hid the 2.11 ACL
            // regression for a month.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                // Give the webview time to load before starting watchdog
                std::thread::sleep(std::time::Duration::from_secs(30));
                let mut watchdog = ReloadWatchdog::default();
                loop {
                    std::thread::sleep(WATCHDOG_TICK);
                    let Some(ww) = handle.get_webview_window("main") else {
                        continue;
                    };
                    let state = handle.state::<LastHeartbeat>();
                    let silent_for = state.at.lock().unwrap().elapsed();
                    let heartbeats = state.count.load(std::sync::atomic::Ordering::Relaxed);
                    let Some(decision) = watchdog.on_tick(silent_for, heartbeats) else {
                        continue;
                    };
                    if decision.futile {
                        eprintln!(
                            "[Tauri] WKWebView heartbeat silent for {:.0}s and the page has not \
                             beaten ONCE since the last reload ({} futile reloads) — reloading \
                             anyway, then backing off to {:.0}s. A reload that never restores the \
                             heartbeat means the page is running but cannot reach us: check the \
                             engine log for [Client/ipc] lines, and check for \"not allowed by \
                             ACL\" rejections.",
                            silent_for.as_secs_f64(),
                            watchdog.futile_reloads,
                            decision.next_threshold.as_secs_f64(),
                        );
                    } else {
                        eprintln!(
                            "[Tauri] WKWebView heartbeat timeout ({:.0}s) — reloading",
                            silent_for.as_secs_f64()
                        );
                    }
                    if let Ok(url) = ww.url() {
                        let _ = ww.navigate(url);
                    }
                    // Reset the clock so the next threshold is measured from this
                    // reload rather than from the last heartbeat.
                    *state.at.lock().unwrap() = Instant::now();
                }
            });

            // Debounced window-geometry flush. The window-state plugin only writes
            // `.window-state.json` to disk on RunEvent::Exit, which the packaged
            // client deliberately never reaches (closing hides the window; exit is
            // prevented while the launchd service runs). Without this, a moved or
            // resized window — including onto another screen — is remembered only
            // in memory and lost on the next launch. This flushes shortly after the
            // user stops dragging/resizing (see should_persist_geometry). The save
            // itself is marshalled onto the main thread (persist_window_state_on_main)
            // — saving from this worker thread directly would deadlock the UI.
            let geometry_handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(300));
                let saver = geometry_handle.state::<GeometrySaver>();
                let dirty = saver.dirty.load(std::sync::atomic::Ordering::Acquire);
                let since = saver.last_change.lock().unwrap().elapsed();
                if should_persist_geometry(dirty, since) {
                    saver
                        .dirty
                        .store(false, std::sync::atomic::Ordering::Release);
                    persist_window_state_on_main(&geometry_handle);
                }
            });

            // Register the UserNotifications delegate + request notification
            // authorization. No-op in dev (unbundled binary). See notifications.rs.
            notifications::setup(app.handle());

            // Packaged build: boot the bundled Postgres + engine and point the
            // window at it. No-op in development (tauri-dev.sh supplies both). The
            // nudge receiver lets the dock-badge loop recompute on demand.
            desktop::launch(app.handle(), dock_badge_nudge_rx);
            // Update detection is surfaced INSIDE the workspace UI: the web app
            // polls the `check_app_update` command and shows an in-app
            // "Update & restart" toast (see updater.rs). No native launch dialog.
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // The engine + Postgres run as a launchd service, independent of the
            // client. Packaged: keep the client process alive when the last window
            // is dismissed (so it can host the menu-bar item and be re-opened), and
            // re-show the window on a Dock click. The explicit teardown
            // (`quit_lucidos`) sets QUITTING so its app.exit(0) passes the guard.
            match event {
                tauri::RunEvent::ExitRequested { api, .. } => {
                    if !tauri::is_dev() && !QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
                        api.prevent_exit();
                    }
                }
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    if !tauri::is_dev() {
                        show_main_window(app_handle);
                    }
                }
                _ => {}
            }
        });
}

/// Build and install the app menu.
///
/// We DERIVE from Tauri's OS-default menu (`Menu::default`) rather than building
/// every submenu by hand. This is load-bearing on macOS: the system only loads
/// the standard text-editing key-binding dictionary for a WKWebView when the app
/// exposes a *complete* native app menu (the default App menu — About, Services,
/// Hide… — is what gets the first submenu recognized as the Apple menu). A fully
/// hand-rolled menu that omitted those predefined items left the bindings
/// inactive, so `interpretKeyEvents:` found no command for the arrow keys and
/// fell back to `insertText:` — typing the raw arrow NSFunctionKey characters
/// (U+F700–F703, rendered as tofu boxes) into the focused textarea instead of
/// moving the cursor. Deriving from the default keeps that wiring; on macOS we
/// then graft on our service-aware items.
///
/// macOS customizations: Cmd+Q maps to "Close to Menu Bar" — it closes every
/// client window and drops the app to the menu-bar tray (out of the Dock +
/// Cmd+Tab) while the always-on service keeps running; per-window close stays the
/// default File/Window "Close Window" (Cmd+W). The deliberate full teardown is the
/// separate, unshortcutted "Quit and Stop Background Service" (`on_menu_event` →
/// `quit_lucidos`; also in the menu-bar tray); plus "Uninstall Lucidos…" and a
/// File-menu "New Window". Other platforms keep the default menu unchanged.
fn install_app_menu(app: &tauri::App) -> tauri::Result<()> {
    let menu = Menu::default(app.handle())?;

    #[cfg(target_os = "macos")]
    {
        use tauri::menu::MenuItemKind;

        // Default macOS submenu order is [App, File, Edit, View, Window, Help].
        let items = menu.items()?;

        if let Some(MenuItemKind::Submenu(app_menu)) = items.first() {
            // Drop the default Quit (its last item; Cmd+Q → terminate) so Cmd+Q is
            // free for "Close to Menu Bar", and graft on our service-aware items.
            // Removing only Quit leaves About/Services/Hide intact, so the menu is
            // still recognized as the Apple menu (the arrow-key fix above). The
            // default's item before Quit is a separator, so the grafted items start
            // straight at Uninstall (no leading separator → no double rule).
            // Per-window close stays the default File/Window "Close Window" (Cmd+W).
            let last = app_menu.items()?.len();
            if last > 0 {
                app_menu.remove_at(last - 1)?;
            }
            let uninstall = MenuItem::with_id(
                app,
                "uninstall_lucidos",
                "Uninstall Lucidos…",
                true,
                None::<&str>,
            )?;
            let close_to_menu_bar = MenuItem::with_id(
                app,
                "close_to_menu_bar",
                "Close to Menu Bar",
                true,
                Some("Cmd+Q"),
            )?;
            let quit = MenuItem::with_id(
                app,
                "quit_lucidos",
                "Quit and Stop Background Service",
                true,
                None::<&str>,
            )?;
            app_menu.append_items(&[
                &uninstall,
                &PredefinedMenuItem::separator(app)?,
                &close_to_menu_bar,
                &quit,
            ])?;
        }

        if let Some(MenuItemKind::Submenu(file_menu)) = items.get(1) {
            let new_window =
                MenuItem::with_id(app, "new_window", "New Window", true, Some("Cmd+N"))?;
            file_menu.prepend(&new_window)?;
        }
    }

    app.set_menu(menu)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    #[test]
    fn should_be_menu_bar_only_iff_no_windows_visible() {
        // No visible app window → drop to the menu-bar tray (Accessory).
        assert!(should_be_menu_bar_only(0));
        // Any visible window → stay a normal Dock app (Regular), incl. the
        // main-hidden-but-a-secondary-still-open case.
        assert!(!should_be_menu_bar_only(1));
        assert!(!should_be_menu_bar_only(3));
    }

    #[test]
    fn unread_targets_routes_by_activation_state() {
        // Regular (a window is open): count on the Dock badge, nothing on the tray.
        assert_eq!(unread_targets(false, 5), (Some("5".into()), None));
        // Menu-bar only (Accessory): count on the tray title, no Dock tile.
        assert_eq!(unread_targets(true, 5), (None, Some("5".into())));
        // Zero clears both surfaces regardless of state.
        assert_eq!(unread_targets(false, 0), (None, None));
        assert_eq!(unread_targets(true, 0), (None, None));
        // Over 99 collapses to "99+" on whichever surface is active.
        assert_eq!(unread_targets(false, 150), (Some("99+".into()), None));
        assert_eq!(unread_targets(true, 100), (None, Some("99+".into())));
        // Boundary: exactly 99 is shown as-is.
        assert_eq!(unread_targets(false, 99), (Some("99".into()), None));
    }

    #[test]
    fn safari_ua_carries_the_version_and_webkit_suffix() {
        let ua = safari_ua("18.5");
        // The whole point: WKWebView's default UA lacks the Version/… Safari/…
        // suffix; ours must carry both, plus the AppleWebKit token.
        assert!(ua.contains("Version/18.5 Safari/605.1.15"), "{ua}");
        assert!(ua.contains("AppleWebKit/605.1.15"), "{ua}");
        assert!(ua.starts_with("Mozilla/5.0 (Macintosh;"), "{ua}");
        // A different version is interpolated verbatim.
        assert!(safari_ua("17.0").contains("Version/17.0 Safari/605.1.15"));
    }

    #[test]
    fn the_watchdog_reloads_only_past_the_timeout() {
        let mut watchdog = ReloadWatchdog::default();
        // Below and at the threshold: no reload (15s heartbeat cadence).
        assert_eq!(watchdog.on_tick(Duration::from_secs(59), 100), None);
        assert_eq!(watchdog.on_tick(HEARTBEAT_TIMEOUT, 100), None);
        // Strictly past the 60s timeout: reload. The first one is never "futile"
        // — there is no earlier reload for it to have failed to improve on.
        assert_eq!(
            watchdog.on_tick(Duration::from_secs(61), 100),
            Some(ReloadDecision {
                futile: false,
                next_threshold: HEARTBEAT_TIMEOUT,
            })
        );
    }

    #[test]
    fn a_reload_that_restores_the_heartbeat_keeps_the_base_interval() {
        let mut watchdog = ReloadWatchdog::default();
        // Genuine content-process crash: reload, page comes back and beats, and
        // some time later it crashes again. Each recovery keeps the fast 60s
        // interval, because reloading is demonstrably working.
        for beats in [10_u64, 25, 40] {
            let decision = watchdog
                .on_tick(HEARTBEAT_TIMEOUT + Duration::from_secs(1), beats)
                .expect("silent past the threshold must reload");
            assert!(!decision.futile);
            assert_eq!(decision.next_threshold, HEARTBEAT_TIMEOUT);
        }
    }

    #[test]
    fn reloads_that_never_restore_the_heartbeat_back_off_instead_of_thrashing() {
        // The 2.11 ACL regression: the page loads and runs, but `invoke` is
        // rejected, so the count NEVER advances. Before the backoff this reloaded
        // once a minute forever (6232 times in the field) and said nothing new.
        let mut watchdog = ReloadWatchdog::default();
        let mut thresholds = Vec::new();
        for _ in 0..8 {
            // Always just past whatever the current threshold is.
            let silent_for = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
            let decision = watchdog
                .on_tick(silent_for, 0)
                .expect("silent past the threshold must reload");
            thresholds.push(decision.next_threshold);
        }
        // First reload is not yet futile; every one after it is, and the interval
        // doubles until it hits the ceiling and stays there.
        assert_eq!(
            thresholds,
            vec![
                HEARTBEAT_TIMEOUT,      // 60s  — first attempt
                HEARTBEAT_TIMEOUT * 2,  // 2m
                HEARTBEAT_TIMEOUT * 4,  // 4m
                HEARTBEAT_TIMEOUT * 8,  // 8m
                HEARTBEAT_TIMEOUT * 16, // 16m
                HEARTBEAT_TIMEOUT * 32, // 32m — ceiling
                HEARTBEAT_TIMEOUT * 32,
                HEARTBEAT_TIMEOUT * 32,
            ]
        );
    }

    #[test]
    fn the_backoff_resets_as_soon_as_the_page_beats_again() {
        // Backing off must not become permanent: if the reloads were futile only
        // because the gateway was down, the page eventually loads and beats, and
        // full-speed crash recovery has to come straight back.
        let mut watchdog = ReloadWatchdog::default();
        for _ in 0..4 {
            let silent_for = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
            watchdog.on_tick(silent_for, 0);
        }
        assert!(watchdog.futile_reloads > 0, "expected to be backed off");
        // One heartbeat arrives, then silence again.
        let decision = watchdog
            .on_tick(
                reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1),
                1,
            )
            .expect("silent past the threshold must reload");
        assert!(!decision.futile);
        assert_eq!(decision.next_threshold, HEARTBEAT_TIMEOUT);
        assert_eq!(watchdog.futile_reloads, 0);
    }

    #[test]
    fn the_backoff_never_stops_retrying() {
        // Deliberately a backoff and not a give-up: the ceiling is finite, so a
        // cause that clears itself hours later is still recovered from.
        assert_eq!(
            reload_threshold(u32::MAX),
            reload_threshold(MAX_RELOAD_BACKOFF_DOUBLINGS)
        );
        assert!(reload_threshold(u32::MAX) <= Duration::from_secs(60 * 60));
    }

    #[test]
    fn should_persist_geometry_waits_for_quiet_then_fires() {
        // Nothing changed → never flush, however long it's been.
        assert!(!should_persist_geometry(false, Duration::from_secs(10)));
        // Dirty but the user is still moving/resizing (within the debounce) → wait.
        assert!(!should_persist_geometry(true, Duration::from_millis(0)));
        assert!(!should_persist_geometry(
            true,
            GEOMETRY_SAVE_DEBOUNCE - Duration::from_millis(1)
        ));
        // Dirty and quiet for at least the debounce window → flush to disk.
        assert!(should_persist_geometry(true, GEOMETRY_SAVE_DEBOUNCE));
        assert!(should_persist_geometry(
            true,
            GEOMETRY_SAVE_DEBOUNCE + Duration::from_millis(1)
        ));
    }

    #[test]
    fn window_state_flags_remembers_geometry_not_visibility() {
        let flags = window_state_flags();
        // The whole point: remember where/how big the window is, on which screen.
        assert!(flags.contains(StateFlags::SIZE));
        assert!(flags.contains(StateFlags::POSITION));
        assert!(flags.contains(StateFlags::MAXIMIZED));
        assert!(flags.contains(StateFlags::FULLSCREEN));
        // But NOT VISIBLE (a flush taken while hidden would restore hidden) and
        // NOT DECORATIONS (toggling it on macOS drops the Overlay title bar).
        assert!(!flags.contains(StateFlags::VISIBLE));
        assert!(!flags.contains(StateFlags::DECORATIONS));
    }
}

/// Regression coverage for the Tauri ACL, which is what stands between the
/// packaged window and every one of our IPC commands.
///
/// These tests drive the REAL resolver — `Resolved::resolve` +
/// `RuntimeAuthority::resolve_access`, the exact pair `Webview::on_message`
/// consults — over the REAL artifacts (`gen/schemas/*.json`, regenerated by
/// `tauri_build` on every build, and the runtime capability from
/// `desktop::gateway_capability`). So they fail for the same reason the packaged
/// app would, without needing a bundle.
///
/// What they cannot prove is the origin STRING the OS produces: tauri derives it
/// from the `Origin` header WebKit puts on the `ipc://` request
/// (`tauri/src/ipc/protocol.rs`, `parse_invoke_request`). That the header for our
/// window really is `http://localhost:<port>` is established by code inspection
/// plus the field evidence, not by these tests.
#[cfg(test)]
mod acl_tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use tauri::ipc::{Origin, RuntimeCapability};
    use tauri::utils::acl::capability::{Capability, CapabilityFile};
    use tauri::utils::acl::manifest::Manifest;
    use tauri::utils::acl::resolved::Resolved;
    use tauri::utils::platform::Target;

    /// The gateway port the tests pin the capability to. Any value works — the
    /// point is that the SAME one has to appear in the window's origin.
    const PORT: u16 = 5252;

    /// Where the packaged window ends up, and therefore the origin every IPC
    /// request from it is judged against.
    fn gateway_origin() -> tauri::Url {
        desktop::gateway_url(PORT).parse().unwrap()
    }

    fn remote(url: &str) -> Origin {
        Origin::Remote {
            url: url.parse().unwrap(),
        }
    }

    /// The app's ACL manifests exactly as compiled into the binary.
    fn acl_manifests() -> BTreeMap<String, Manifest> {
        serde_json::from_str(include_str!("../gen/schemas/acl-manifests.json"))
            .expect("gen/schemas/acl-manifests.json is generated by tauri_build")
    }

    /// The static capabilities exactly as compiled into the binary.
    fn static_capabilities() -> BTreeMap<String, Capability> {
        serde_json::from_str(include_str!("../gen/schemas/capabilities.json"))
            .expect("gen/schemas/capabilities.json is generated by tauri_build")
    }

    /// The runtime capability `desktop::launch` registers, as a plain
    /// [`Capability`] the resolver can consume.
    fn gateway_capability(port: u16) -> Capability {
        match desktop::gateway_capability(port).build() {
            CapabilityFile::Capability(c) => c,
            _ => panic!("gateway_capability must build exactly one capability"),
        }
    }

    /// Build the authority the packaged client runs with: the static capabilities
    /// plus, when `gateway_port` is set, the runtime capability `desktop::launch`
    /// registers before navigating. Passing `None` reproduces the pre-fix state.
    fn authority(gateway_port: Option<u16>) -> tauri::ipc::RuntimeAuthority {
        let acl = acl_manifests();
        let mut capabilities = static_capabilities();
        if let Some(port) = gateway_port {
            let capability = gateway_capability(port);
            capabilities.insert(capability.identifier.clone(), capability);
        }
        let resolved = Resolved::resolve(&acl, capabilities, Target::current())
            .expect("capabilities must resolve against the app's ACL manifests");
        tauri::runtime_authority!(acl, resolved)
    }

    /// Command names registered in `tauri::generate_handler!`, read out of this
    /// very file so the list can never be a stale copy. Module qualifiers are
    /// stripped: tauri registers `updater::check_app_update` under the bare
    /// function name, which is also how the ACL keys it.
    fn invoke_handler_commands() -> BTreeSet<String> {
        let source = include_str!("lib.rs");
        let (_, after) = source
            .split_once("tauri::generate_handler![")
            .expect("run() registers commands with tauri::generate_handler!");
        let (block, _) = after
            .split_once(']')
            .expect("the generate_handler! block must be closed");
        let commands: BTreeSet<String> = block
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.rsplit("::").next().unwrap().to_string())
            .collect();
        // Without this, a parse that silently yielded nothing would make every
        // "each command is allowed" assertion below pass over an empty set.
        assert!(
            !commands.is_empty(),
            "parsed no commands out of the generate_handler! block"
        );
        commands
    }

    /// The commands one of our app permissions allows.
    fn permission_commands(identifier: &str) -> BTreeSet<String> {
        let file: serde_json::Value =
            serde_json::from_str(include_str!("../permissions/app-ipc.json")).unwrap();
        let permission = file["permission"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["identifier"] == identifier)
            .unwrap_or_else(|| panic!("permissions/app-ipc.json defines no {identifier}"));
        permission["commands"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect()
    }

    /// Plugin commands the frontend depends on, keyed the way the ACL keys them.
    /// `plugin:event|listen` is the load-bearing one: every `listen()` in the
    /// frontend — native-notification taps, panel title/URL updates — goes
    /// through it, so a denial there is silent and total.
    const PLUGIN_COMMANDS: &[&str] = &[
        "plugin:event|listen",
        "plugin:event|unlisten",
        "plugin:webview|create_webview",
        "plugin:dialog|message",
        "plugin:updater|check",
        "plugin:updater|download_and_install",
    ];

    #[test]
    fn the_app_defines_an_acl_manifest() {
        // With no app ACL manifest, `has_app_acl_manifest` is false and app
        // commands are unchecked on LOCAL origins — which is exactly the state
        // that let the 2.11 remote-origin check break everything at once, because
        // nothing had ever had to declare an app permission. Its presence is what
        // makes every assertion below meaningful.
        assert!(
            acl_manifests().contains_key(tauri::utils::acl::APP_ACL_KEY),
            "permissions/app-ipc.json must produce an app ACL manifest"
        );
    }

    #[test]
    fn every_invoke_handler_command_is_allowed_from_the_gateway_origin() {
        let authority = authority(Some(PORT));
        let origin = Origin::Remote {
            url: gateway_origin(),
        };
        for command in invoke_handler_commands() {
            // The panel reports are invoked from the previewed page, never from
            // our own frontend, and are covered by their own test below.
            if command.starts_with("__panel_") {
                continue;
            }
            assert!(
                authority
                    .resolve_access(&command, "main", "main", &origin)
                    .is_some(),
                "`{command}` is denied on the packaged window — the ACL would reject it with \
                 \"Command {command} not allowed by ACL\""
            );
        }
    }

    #[test]
    fn declared_plugin_permissions_reach_the_gateway_origin_too() {
        let authority = authority(Some(PORT));
        let origin = Origin::Remote {
            url: gateway_origin(),
        };
        for command in PLUGIN_COMMANDS {
            assert!(
                authority
                    .resolve_access(command, "main", "main", &origin)
                    .is_some(),
                "plugin command `{command}` is denied on the packaged window"
            );
        }
    }

    #[test]
    fn secondary_app_windows_get_the_same_access_as_main() {
        // "New Window" builds `window-N` and points it at the main window's URL,
        // so it lands on the gateway origin too and needs the same grants.
        let authority = authority(Some(PORT));
        let origin = Origin::Remote {
            url: gateway_origin(),
        };
        for (window, webview) in [("window-0", "window-0"), ("window-42", "window-42")] {
            assert!(
                authority
                    .resolve_access("heartbeat", window, webview, &origin)
                    .is_some(),
                "`heartbeat` is denied on {webview}"
            );
            assert!(
                authority
                    .resolve_access("plugin:event|listen", window, webview, &origin)
                    .is_some(),
                "`plugin:event|listen` is denied on {webview}"
            );
        }
    }

    #[test]
    fn without_the_gateway_capability_the_gateway_origin_is_denied() {
        // The regression itself: static capabilities alone (`local: true`, no
        // remote context) leave every command denied once the window leaves the
        // Tauri asset origin. This is the state the shipped v0.16.0 is in.
        let authority = authority(None);
        let origin = Origin::Remote {
            url: gateway_origin(),
        };
        for command in [
            "heartbeat",
            "show_native_notification",
            "plugin:event|listen",
        ] {
            assert!(
                authority
                    .resolve_access(command, "main", "main", &origin)
                    .is_none(),
                "`{command}` resolved without the gateway capability — this test no longer \
                 reproduces the regression it guards"
            );
        }
    }

    #[test]
    fn the_gateway_capability_admits_only_the_resolved_origin() {
        let authority = authority(Some(PORT));
        // A different local port (another server on this machine), a foreign
        // host, and a different scheme must all stay out: the whole point of the
        // upstream 2.11 change is that remote content can't reach custom commands.
        for url in [
            "http://localhost:9999",
            "http://127.0.0.1:5252",
            "http://evil.example",
            "http://localhost.evil.example:5252",
            "https://localhost:5252",
        ] {
            assert!(
                authority
                    .resolve_access("heartbeat", "main", "main", &remote(url))
                    .is_none(),
                "`heartbeat` is reachable from {url} — the gateway URL pattern is too loose"
            );
        }
    }

    #[test]
    fn panel_previews_get_the_three_report_commands_and_nothing_else() {
        let authority = authority(Some(PORT));
        // A preview webview lives INSIDE the main window and shows an arbitrary
        // third-party page, so it is judged as (window: main, webview:
        // url-preview-N) on that page's origin.
        let previewed = remote("https://example.com");
        for command in [
            "__panel_title_report",
            "__panel_url_report",
            "__panel_content_report",
        ] {
            assert!(
                authority
                    .resolve_access(command, "main", "url-preview-3", &previewed)
                    .is_some(),
                "`{command}` is denied in a panel preview — the previewed page's title/URL/text \
                 would never reach the main window"
            );
        }
        // Everything else stays out of reach of previewed content. Before 2.11
        // (no app ACL manifest) all of these were callable from any previewed page.
        for command in [
            "heartbeat",
            "restart_app",
            "quit_lucidos",
            "uninstall_lucidos",
            "open_url_external",
            "show_native_notification",
            "get_or_create_device_id",
            "plugin:event|listen",
        ] {
            assert!(
                authority
                    .resolve_access(command, "main", "url-preview-3", &previewed)
                    .is_none(),
                "`{command}` is reachable from arbitrary previewed content"
            );
        }
    }

    #[test]
    fn the_app_surface_is_not_reachable_from_a_preview_on_the_gateway_origin() {
        // Same-origin previews are the sharp edge of scoping by `windows`: if the
        // gateway capability were window-scoped, a preview of a localhost page
        // would inherit the main window's full grant.
        let authority = authority(Some(PORT));
        let origin = Origin::Remote {
            url: gateway_origin(),
        };
        assert!(
            authority
                .resolve_access("heartbeat", "main", "url-preview-1", &origin)
                .is_none(),
            "a preview webview inherited the main window's app IPC grant"
        );
    }

    #[test]
    fn dev_keeps_working_on_the_local_origin() {
        // Defining an app ACL manifest switches `has_app_acl_manifest` to true,
        // which starts ACL-checking app commands on LOCAL origins too — dev
        // (`devUrl`) and the bundled `tauri://localhost` splash included. Without
        // `allow-app-ipc` in the default capability this would break `tauri dev`
        // and the splash's own heartbeat.
        let authority = authority(None);
        for command in invoke_handler_commands() {
            if command.starts_with("__panel_") {
                continue;
            }
            assert!(
                authority
                    .resolve_access(&command, "main", "main", &Origin::Local)
                    .is_some(),
                "`{command}` is denied on the local app URL — dev and the boot splash are broken"
            );
        }
        for command in PLUGIN_COMMANDS {
            assert!(
                authority
                    .resolve_access(command, "main", "main", &Origin::Local)
                    .is_some(),
                "plugin command `{command}` is denied on the local app URL"
            );
        }
    }

    #[test]
    fn app_permissions_match_the_invoke_handler() {
        // The app ACL manifest makes an omission fatal: a command registered in
        // `generate_handler!` but absent from permissions/app-ipc.json is denied
        // on EVERY origin, dev included. This is the guard that turns that into a
        // test failure instead of a shipped regression.
        let registered = invoke_handler_commands();
        let permitted: BTreeSet<String> = permission_commands("allow-app-ipc")
            .union(&permission_commands("allow-panel-report"))
            .cloned()
            .collect();
        let unpermitted: Vec<_> = registered.difference(&permitted).collect();
        assert!(
            unpermitted.is_empty(),
            "registered with generate_handler! but not allowed by permissions/app-ipc.json: \
             {unpermitted:?} — they would be rejected by the ACL everywhere"
        );
        let stale: Vec<_> = permitted.difference(&registered).collect();
        assert!(
            stale.is_empty(),
            "allowed by permissions/app-ipc.json but no longer registered: {stale:?}"
        );
        // The split matters: the panel permission is granted to arbitrary remote
        // content, so it must stay exactly the three report commands.
        assert_eq!(
            permission_commands("allow-panel-report"),
            BTreeSet::from([
                "__panel_title_report".to_string(),
                "__panel_url_report".to_string(),
                "__panel_content_report".to_string(),
            ]),
            "allow-panel-report is granted to ANY remote origin — keep it to the three reports"
        );
    }

    #[test]
    fn gateway_capability_grants_what_the_default_capability_grants() {
        // Same frontend, one reached over http and one off the bundled assets —
        // if the two permission lists drift, a command works in dev and dies in
        // the packaged build (or the reverse), which is precisely the class of
        // bug that went unnoticed for a month.
        let default_permissions: BTreeSet<String> = static_capabilities()["default"]
            .permissions
            .iter()
            .map(|p| p.identifier().get().to_string())
            .collect();
        let gateway_permissions: BTreeSet<String> = gateway_capability(PORT)
            .permissions
            .iter()
            .map(|p| p.identifier().get().to_string())
            .collect();
        assert_eq!(gateway_permissions, default_permissions);
    }

    #[test]
    fn no_capability_mixes_a_remote_context_with_window_scoping() {
        // `windows` enables a capability on EVERY webview of the matching window,
        // and the `url-preview-*` webviews showing third-party sites live inside
        // the main window. Combined with a remote context that would hand our IPC
        // surface to previewed content. Webview scoping is the only safe form.
        let mut capabilities = static_capabilities();
        let capability = gateway_capability(PORT);
        capabilities.insert(capability.identifier.clone(), capability);
        for (identifier, capability) in &capabilities {
            if capability.remote.is_some() {
                assert!(
                    capability.windows.is_empty(),
                    "capability `{identifier}` has a remote context AND window scoping"
                );
                assert!(
                    !capability.webviews.is_empty(),
                    "capability `{identifier}` has a remote context but no webview scoping, so it \
                     applies to every webview in the app"
                );
            }
        }
    }
}
