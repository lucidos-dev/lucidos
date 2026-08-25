use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Mutex;
use std::time::Instant;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::PageLoadEvent;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

mod config_scalar;
mod desktop;
mod device_id_store;
mod mobile;
mod notifications;
mod pairing;
/// Login-shell environment hydration for a GUI launch. macOS-only: it exists
/// because launchd hands a packaged process an environment the user's profile
/// never touched, which is a macOS packaging fact.
#[cfg(target_os = "macos")]
mod shell_env;
mod traffic_lights;
mod updater;
mod window_restore;

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

/// Hand `url` to the OS default handler.
///
/// Errs rather than logging, because every caller is user-initiated and every JS
/// caller of `openExternal` already toasts the rejection. A missing launcher
/// otherwise reads to the user as the button doing nothing.
///
/// The spawn failure is the ONLY failure observable here. The launcher is a
/// fire-and-forget child, so one that starts and then fails exits after this has
/// returned. Waiting on it would block the main thread, since a synchronous
/// `#[tauri::command]` runs there.
fn open_in_default_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let cmd = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "linux")]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();

    cmd.map(|_| ())
        .map_err(|e| format!("could not start the system opener: {e}"))
}

/// Channel for receiving page content extracted from the panel webview.
struct PanelContentChannel(Mutex<Option<std::sync::mpsc::Sender<(String, String)>>>);

/// Tracks the label of the currently active panel webview.
struct PanelWebview(Mutex<Option<String>>);

/// Sender that wakes the unread-indicator loop (`desktop::launch`) for an
/// immediate recompute, so the tray title and dock badge update the instant a
/// notification is read rather than on the next poll tick. A send is a harmless
/// no-op where there is no consumer (dev, and off macOS).
///
/// The "dock badge" in this name is a wire contract with the frontend: it is
/// spelled in `permissions/app-ipc.json`, in the generated
/// `gen/schemas/acl-manifests.json`, and in `utils/tauri.ts`. What it nudges is
/// the unread indicator as a whole.
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
/// the base interval is how a broken IPC bridge becomes a silent reload every
/// minute for weeks. Backing off rather than giving up kills the thrash while
/// still recovering if the cause was temporary.
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

/// Window state the app persists and restores via `tauri-plugin-window-state`.
/// Deliberately EXCLUDES two flags:
/// - `VISIBLE`: the packaged client hides its window rather than closing it. A
///   flush taken while hidden would persist `visible: false`, and the plugin
///   would restore the window hidden on the next launch.
/// - `DECORATIONS`: toggling it on macOS rebuilds the NSWindow style mask and
///   can drop the `titleBarStyle: "Overlay"` configuration, turning the
///   reclaimed title-bar band back into an opaque bar.
fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
}

/// How long the window must sit still (no move/resize) before the debounced
/// background flush writes `.window-state.json`. Short enough that a quick
/// move-then-relaunch is remembered, long enough that a drag doesn't thrash the
/// disk on every intermediate `Moved`/`Resized` event.
const GEOMETRY_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(600);

/// Coordinates the debounced geometry flush. The window-state plugin writes to
/// disk only on `RunEvent::Exit`, which the packaged client never reaches.
/// Without this, a moved or resized window is remembered in memory alone. A
/// background thread flushes once the window has been quiet for
/// [`GEOMETRY_SAVE_DEBOUNCE`] (see [`should_persist_geometry`]).
struct GeometrySaver {
    dirty: AtomicBool,
    last_change: Mutex<Instant>,
}

/// Whether the debounced flush should run now: there is unsaved geometry and
/// the window has been quiet at least [`GEOMETRY_SAVE_DEBOUNCE`].
fn should_persist_geometry(dirty: bool, since_last_change: std::time::Duration) -> bool {
    dirty && since_last_change >= GEOMETRY_SAVE_DEBOUNCE
}

/// Persist window geometry, forcing the work onto the MAIN thread.
///
/// `tauri-plugin-window-state::save_window_state` holds an internal cache lock
/// while it reads each window's live geometry. Off the main thread those getters
/// block on a round-trip to the event loop. So a worker-thread save holds the
/// cache lock across a wait for the main thread, while the main thread blocks
/// taking that same lock: a full-UI deadlock. Every caller NOT already on the
/// main thread must route through here. Fire-and-forget, so the save runs on the
/// next main-loop turn.
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

/// Label of the app window declared in `tauri.conf.json`. It is the one window
/// a packaged close only HIDES (so it can be reshown instantly), which is why it
/// is also the window every "bring the client forward" path prefers.
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

/// Label prefix for additional top-level app windows opened via File → New
/// Window. The first window is `main` (declared in `tauri.conf.json`); each
/// extra window gets `window-<n>`. Panel preview webviews use the
/// `url-preview-<n>` prefix instead, so app-window-only setup (the app-version
/// injection in `on_page_load`) can tell the two apart.
const APP_WINDOW_PREFIX: &str = "window-";

/// Default macOS window background tint, the dark-theme header-top blue. Under
/// `titleBarStyle: "Overlay"` the webview paints the reclaimed title-bar band
/// itself. This NSWindow background is the behind-the-webview fallback, so the
/// band reads blue rather than black before the page paints.
///
/// The FALLBACK only: once the frontend has reported a color it is remembered
/// (see [`TITLE_BAR_COLOR_FILE`]), so this constant covers a first run and an
/// unreadable file.
const TITLE_BAR_DEFAULT_COLOR: &str = "#15549e";

/// JS appended to every app window's startup injection, and empty off macOS.
/// It stamps three facts pre-paint, so the header's first frame lays out right.
///
/// `--titlebar-inset` is the macOS title-bar height the `.titlebar-strip`
/// element sizes to. `--titlebar-lights-x` is the x
/// [`traffic_lights::place`] puts the buttons at, which is what makes
/// `--titlebar-lights-reserve` in `styles/panels/shell.css` derived rather than
/// an independent guess.
///
/// `data-titlebar-overlay` is the same fact as a SELECTOR rather than a length,
/// and two rules need it that way. The header's leading control steps right by
/// the reserve, which has a fallback x and so resolves even unstamped: keyed on
/// a var, a web header would be indented by lights it does not have. And the
/// header is SHORTENED by the band, a subtraction that is only correct where a
/// band exists.
fn titlebar_inset_script() -> String {
    #[cfg(target_os = "macos")]
    {
        format!(
            "if(document.documentElement){{\
             document.documentElement.style.setProperty('--titlebar-inset','28px');\
             document.documentElement.style.setProperty('--titlebar-lights-x','{}px');\
             document.documentElement.setAttribute('data-titlebar-overlay','');}}",
            traffic_lights::LIGHTS_X_PX
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        String::new()
    }
}

fn get_main_window(app: &tauri::AppHandle) -> Option<tauri::Window> {
    if let Some(w) = app.get_window("main") {
        return Some(w);
    }
    app.get_webview_window("main")
        .map(|ww| ww.as_ref().window().clone())
}

/// True if `label` names a top-level Lucidos app window (the declared `main` or
/// a New-Window child), as opposed to a `url-preview-*` panel webview.
pub(crate) fn is_app_window(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL || label.starts_with(APP_WINDOW_PREFIX)
}

/// Paint the macOS window background of every top-level app window the given
/// color. Sets the WINDOW-layer background only, never the webview's, so the
/// page background is never tinted and there is no load flash. Panel preview
/// webviews are skipped, and a failure on one window is logged, not fatal.
fn paint_title_bars(app: &tauri::AppHandle, color: tauri::utils::config::Color) {
    for (label, window) in app.windows() {
        if is_app_window(&label) {
            if let Err(e) = window.set_background_color(Some(color)) {
                eprintln!("[Tauri] Failed to set title-bar color on {label}: {e}");
            }
        }
    }
}

/// Frontend-driven window-background tint. `applyTheme` calls this with the
/// header-top blue for the active theme, so the behind-the-webview fallback
/// tracks the in-app header across theme switches. `color` is a CSS hex string.
#[tauri::command]
fn set_titlebar_color(app: tauri::AppHandle, color: String) -> Result<(), String> {
    let parsed = color
        .parse()
        .map_err(|e| format!("invalid color {color:?}: {e}"))?;
    paint_title_bars(&app, parsed);
    // Remember it for the NEXT launch's pre-paint tint. Deliberately AFTER the
    // parse, so the file can only ever hold a value the startup path accepts.
    persist_title_bar_color(&app, &color);
    Ok(())
}

/// Remembers the last color the frontend asked for, so a cold launch paints the
/// window background in the user's theme rather than the compiled default. A
/// bare hex string, read back through the same color parse as any other.
/// [`config_scalar`] owns the file plumbing.
const TITLE_BAR_COLOR_FILE: &str = "titlebar-color";

/// Write `color` for the next launch. Trimmed first, because the color parser
/// tolerates surrounding whitespace while [`config_scalar::read`] strips it.
/// Storing the raw string would leave a value that never compares equal to what
/// is read back, so every theme apply would rewrite the file.
fn persist_title_bar_color(app: &tauri::AppHandle, color: &str) {
    config_scalar::write_if_changed(app, TITLE_BAR_COLOR_FILE, color.trim(), "title-bar color");
}

/// Which color string to paint with before a page can report its theme.
/// `persisted` wins only if it still parses as a color. A truncated or
/// hand-edited file degrades to [`TITLE_BAR_DEFAULT_COLOR`] rather than to no
/// tint at all.
fn title_bar_color_or_default(persisted: Option<&str>) -> &str {
    persisted
        .filter(|c| c.parse::<tauri::utils::config::Color>().is_ok())
        .unwrap_or(TITLE_BAR_DEFAULT_COLOR)
}

/// The pre-paint tint for a window that has not reported a theme yet: the color
/// the frontend last asked for, else the compiled default. Used at startup and
/// when a New-Window child is built, the two moments a window exists with
/// nothing painted in it.
fn pre_paint_title_bar_color(app: &tauri::AppHandle) -> Option<tauri::utils::config::Color> {
    let persisted = config_scalar::path(app, TITLE_BAR_COLOR_FILE)
        .as_deref()
        .and_then(config_scalar::read);
    title_bar_color_or_default(persisted.as_deref())
        .parse()
        .ok()
}

/// Frontend-driven traffic-light placement. The app calls this with the height
/// of the header bar it just measured, and the macOS window buttons are centred
/// on that bar. See [`traffic_lights`] for the arithmetic.
///
/// The bar height has to come from the page, because only the page knows it:
/// `--desktop-bar-height` is `3rem` against the user's UI-scale root font size,
/// and the Style Remote retunes it live over SSE. That is why this is a command
/// rather than a value fixed when the window was built.
///
/// `window` is the calling window, so a New-Window child places its own lights.
#[tauri::command]
fn set_traffic_light_offset(
    app: tauri::AppHandle,
    window: tauri::Window,
    bar_height_px: f64,
) -> Result<(), String> {
    traffic_lights::set_bar_height(&app, &window, bar_height_px)
}

/// Start a native window drag for the calling window. `useWindowDragRegion`
/// calls this once the pointer crosses a small movement threshold, so plain
/// clicks still reach the page's own handlers. An app command rather than
/// `data-tauri-drag-region`, whose internal `plugin:window|start_dragging` IPC
/// the capability ACL denies.
#[tauri::command]
fn start_window_drag(window: tauri::Window) -> Result<(), String> {
    window.start_dragging().map_err(|e| format!("{e}"))
}

/// Toggle the calling window between maximized and restored. Bound to a
/// double-click on the reclaimed title-bar strip only, since the header keeps
/// its own double-click. An app command, like `start_window_drag`, so the
/// window-plugin ACL does not apply.
#[tauri::command]
fn toggle_window_maximize(window: tauri::Window) -> Result<(), String> {
    if window.is_maximized().map_err(|e| format!("{e}"))? {
        window.unmaximize().map_err(|e| format!("{e}"))
    } else {
        window.maximize().map_err(|e| format!("{e}"))
    }
}

/// Open an additional top-level app window (File → New Window / Cmd+N) on the
/// window the user is looking at. Every window is just another client of the
/// same engine (the engine + Postgres run as a shared launchd service, see
/// `desktop`), so all windows share one workspace stack. The WKWebView
/// crash-recovery watchdog stays scoped to `main`.
fn open_new_window(app: &tauri::AppHandle) -> Result<(), String> {
    open_app_window(app, new_window_url(app))
}

/// Build a top-level app window at `url`. The one builder every extra window
/// goes through, so a window opened for a notification tap is identical to a
/// File → New Window one: same `window-<n>` label (which is what
/// `desktop::gateway_capability` scopes IPC to), same title-bar style, same
/// pre-paint tint and traffic-light placement.
fn open_app_window(app: &tauri::AppHandle, url: WebviewUrl) -> Result<(), String> {
    let counter = WEBVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    let builder = WebviewWindowBuilder::new(app, &label, url)
        .title("Lucidos")
        .inner_size(1024.0, 768.0)
        .disable_drag_drop_handler();
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    builder.build().map_err(|e| format!("{e}"))?;
    // Tint the bar now, so it is not black for the moment before this window's
    // frontend boots and calls `set_titlebar_color`. `build()` has registered
    // the window, so `paint_title_bars` covers it.
    if let Some(color) = pre_paint_title_bar_color(app) {
        paint_title_bars(app, color);
    }
    // Same for the traffic lights, at the remembered bar height rather than
    // centred for the default scale.
    traffic_lights::place_all(app);
    Ok(())
}

/// The URL a freshly opened app window should load. Mirrors the main window's
/// current URL once it has navigated to the gateway, so the new window lands on
/// the workspace the user is viewing. Falls back to the gateway on the stable
/// packaged port, or to the bundled entry in dev.
fn new_window_url(app: &tauri::AppHandle) -> WebviewUrl {
    if let Some(url) = app.get_webview_window("main").and_then(|w| w.url().ok()) {
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

/// The URL a second window on `workspace` should load, or `None` when
/// `workspace` is not a slug the gateway would serve.
///
/// The caller's OWN origin is preferred, because that is what the web path does:
/// both workspace lists reach a workspace as the origin-relative `/<slug>/`. So a
/// client reached over a tailnet address opens its second window there too. A
/// dev window on the vite port stays put. `fallback_origin` covers a caller on no
/// http(s) URL at all, i.e. the bundled asset scheme before `desktop::launch` has
/// navigated it.
///
/// Pure, so the whole rule is unit-testable without a window.
fn workspace_window_url(
    caller_url: Option<&str>,
    workspace: &str,
    fallback_origin: &str,
) -> Option<String> {
    if !notifications::is_workspace_slug(workspace) {
        return None;
    }
    let origin = caller_url
        .and_then(notifications::window_origin)
        .unwrap_or(fallback_origin);
    Some(notifications::workspace_url(origin, workspace))
}

/// Open `workspace` in a NEW top-level app window: what "Open in new window"
/// does on a workspace row, in the gateway picker and in the Lucidos menu's
/// switcher.
///
/// A command exists at all because the web answer is unavailable here. A browser
/// opens a tab with `window.open`, which WKWebView drops: wry installs a
/// new-window delegate only when a builder calls `.on_new_window()`, and no app
/// window does.
///
/// It takes a SLUG, never a URL, and composes the URL itself. Every `window-*`
/// webview holds the full IPC permission set on the gateway origin (ADR 0028). A
/// URL chosen by the page would therefore be the page choosing what loads in a
/// window carrying that grant.
#[tauri::command]
fn open_workspace_window(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    workspace: String,
) -> Result<(), String> {
    let caller = window.url().ok().map(|u| u.to_string());
    let fallback = desktop::gateway_url(desktop::engine_port());
    let url = workspace_window_url(caller.as_deref(), &workspace, &fallback)
        .ok_or_else(|| format!("{workspace:?} is not a workspace"))?;
    let parsed = url
        .parse::<tauri::Url>()
        .map_err(|e| format!("could not open {url}: {e}"))?;
    open_app_window(&app, WebviewUrl::External(parsed))
}

/// The difference between Tauri's window logical height and the CSS viewport
/// height the frontend reported.
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
            // The one site with nowhere to report to: a previewed page asked for
            // a window from inside the delegate, so there is no promise to reject
            // and no toast to raise. Log rather than discard.
            if let Err(e) = open_in_default_browser(url.as_str()) {
                eprintln!("[Tauri] {url}: {e}");
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .on_page_load(move |wv, payload| {
            // Fires only for MAIN FRAME navigations.
            match payload.event() {
                PageLoadEvent::Started => {
                    // Grab the title early from <head>, to cut the visible delay.
                    if let Err(e) = wv.eval(TITLE_OBSERVER_JS) {
                        eprintln!("[Tauri] Failed to inject title observer: {e}");
                    }
                }
                PageLoadEvent::Finished => {
                    let url = payload.url().to_string();
                    let _ = page_load_app.emit_to("main", "panel-url-changed", url);
                    // Re-inject for the final title, and observe SPA changes.
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

/// Reports the title immediately, then observes SPA title changes with a
/// `MutationObserver` on `<title>`, and on `<head>` for a late-appearing one.
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

/// Reports URL changes from back and forward navigation, and from SPA routing.
/// Those do not trigger WKWebView's `on_page_load`, so without this the
/// frontend's `panelUrl` drifts out of sync.
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

/// Extract the text content and title from the panel webview. Evals JS that
/// calls `__panel_content_report`, which resolves a sync channel. Runs on a
/// blocking thread, so the main thread is free while it waits.
#[tauri::command]
async fn webview_get_content(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let wv = get_panel_webview(&app).ok_or("panel webview not found")?;

    let (tx, rx) = std::sync::mpsc::channel();
    {
        let state = app.state::<PanelContentChannel>();
        *state.0.lock().unwrap() = Some(tx);
    }

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
/// direct respawn does not). Every other case falls back to replacing this
/// process with a fresh one.
///
/// Sync command, so it runs on the main thread: both `save_window_state` and
/// `cleanup_before_exit` require that.
#[tauri::command]
fn restart_app(app: tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Failed to get current exe: {e}"))?;
    // Minus `--login`: a restart must come back with its window, even for a
    // client that originally started at login.
    let args = desktop::relaunch_args();

    eprintln!("[Tauri] Restarting app: {:?} {:?}", exe, args);

    // The plugin's exit-time flush never runs on the exec path, so without this
    // an in-session move or resize is lost across the restart.
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
    restart_process(&exe, &args)
}

/// Save the window geometry, then exit the client, from the MAIN thread, and
/// never come back.
///
/// For a caller on the async runtime that has already arranged its own relaunch
/// ([`desktop::schedule_relaunch_after_exit`]). Both halves need the main
/// thread: `save_window_state` deadlocks off it (see
/// [`persist_window_state_on_main`]), and `cleanup_before_exit` tears down
/// webviews.
///
/// The save is explicit because `cleanup_before_exit` does NOT dispatch
/// `RunEvent::Exit`, which is what drives the plugin's own exit-time write. It
/// runs inline rather than through [`persist_window_state_on_main`], whose
/// posted main-thread task would never be reached: this closure exits first.
///
/// Parks instead of returning. The caller's next move would be its fallback
/// respawn, which would launch a second client alongside the one the watcher is
/// about to bring up.
pub(crate) fn exit_after_relaunch_scheduled(app: &tauri::AppHandle) -> ! {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Err(e) = handle.save_window_state(window_state_flags()) {
            eprintln!("[Tauri] Failed to persist window state before relaunch: {e}");
        }
        handle.cleanup_before_exit();
        std::process::exit(0);
    }) {
        // The event loop is unreachable, so nothing will run the clean exit.
        // Geometry is already on disk as of the last debounced flush. A client
        // that refuses to die leaves the watcher activating the version we just
        // replaced.
        eprintln!("[Tauri] Could not marshal the exit onto the main thread: {e}");
        std::process::exit(0);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// Open a URL in the system default browser rather than the embedded webview.
/// Reached by every "leave the shell" path. Rejects when the launcher could not
/// be started, which the callers turn into a toast.
#[tauri::command]
fn open_url_external(url: String) -> Result<(), String> {
    open_in_default_browser(&url)
}

/// Where a saved download landed: the folder to open, and the file in it.
#[derive(Serialize)]
struct SavedDownload {
    dir: String,
    path: String,
}

/// Write `contents` into the OS downloads folder as `filename`, and report
/// where it landed.
///
/// The page cannot use the webview's own download machinery. wry attaches a
/// `WKDownloadDelegate` only when the app registers a download handler, and
/// this app registers none, so an `<a download>` click is silently abandoned.
/// Registering one would mean building the `main` window here rather than
/// declaring it in `tauri.conf.json`. Saving through a command keeps that
/// config untouched, and hands the caller the exact destination, which is what
/// lets the toast open the folder.
///
/// `filename` must be a leaf name, so this can only write inside the downloads
/// folder. An existing file is never overwritten: the name takes a ` (1)`
/// counter, matching what a real download would have done.
///
/// macOS gates that folder behind TCC, so the first save raises a system
/// prompt. A denial comes back as `Operation not permitted` inside the write
/// error, which the caller toasts rather than swallowing.
///
/// That prompt is also why the command is `async` while the body is not. Tauri
/// runs a plain sync command on the MAIN thread, so the open would hold the UI
/// up for as long as the dialog. `async` moves the whole body onto the async
/// runtime, where a blocking wait costs nothing on screen.
#[tauri::command(async)]
fn save_to_downloads(
    app: tauri::AppHandle,
    filename: String,
    contents: String,
) -> Result<SavedDownload, String> {
    let dir = app
        .path()
        .download_dir()
        .map_err(|e| format!("could not resolve the downloads folder: {e}"))?;
    let leaf = leaf_filename(&filename)
        .ok_or_else(|| format!("{filename:?} is not a usable file name"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let path = write_new_download(&dir, &leaf, &contents)
        .map_err(|e| format!("could not write {leaf} to {}: {e}", dir.display()))?;
    Ok(SavedDownload {
        dir: dir.display().to_string(),
        path: path.display().to_string(),
    })
}

/// `name` as a single file name, or None when it is anything else.
///
/// Refused rather than repaired, because a repaired name is not the one the
/// caller asked for. The component walk is what makes this platform-correct: it
/// rejects a separator, a `..`, an absolute path, and a Windows drive prefix,
/// each of which would let `Path::join` escape the folder. A leading dot goes
/// too, so a download cannot land hidden.
///
/// The backslash is checked by hand because the walk cannot: Unix takes it as
/// an ordinary character, so `sub\dir.json` is one component there and a
/// traversal on Windows. One rule on every platform is worth more than the
/// literal file name it costs.
fn leaf_filename(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.starts_with('.') || trimmed.contains('\\') {
        return None;
    }
    let mut components = std::path::Path::new(trimmed).components();
    let leaf = match components.next() {
        Some(std::path::Component::Normal(leaf)) => leaf.to_str()?,
        _ => return None,
    };
    components.next().is_none().then(|| leaf.to_string())
}

/// Write `contents` into `dir` as `name`, or as the first free `name (1)`,
/// `name (2)` variant, and report the path taken. The counter sits before the
/// extension, where a browser download puts it.
///
/// Creation is atomic (`create_new`) rather than an existence check followed by
/// a write. Two exports of one thread racing would both find the same name
/// free, and the second write would truncate the first. Here the loser is told
/// `AlreadyExists` and moves to the next counter.
fn write_new_download(
    dir: &std::path::Path,
    name: &str,
    contents: &str,
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    let mut counter = 0u32;
    loop {
        let path = match counter {
            0 => dir.join(name),
            n => dir.join(format!("{stem} ({n}){ext}")),
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(contents.as_bytes()) {
                    // `create_new` just made this path, so nothing that existed
                    // before is at risk. Leaving it would put a TRUNCATED file
                    // under the very name the export was asked for, and the
                    // retry would land beside it as ` (1)`. This export exists
                    // to be attached to a bug report, so the wrong one being
                    // the obvious one to grab is the whole cost.
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => counter += 1,
            Err(e) => return Err(e),
        }
    }
}

/// Restart the always-on gateway service via launchd. The supervisor catches
/// the SIGTERM, tears the bundled gateway and its spawned workspace engines
/// down gracefully, and launchd respawns the service.
#[tauri::command]
fn restart_service() -> Result<(), String> {
    desktop::restart_service()
}

/// Full teardown: confirm with the user, then stop the always-on gateway
/// service and exit the client. This is the ONLY path that stops the service.
/// Closing the window merely hides the client, so triggers, scheduled tasks,
/// coding-agent sessions and push keep running.
///
/// Stopping the service silently halts all of that. So a native confirm spells
/// out the consequence and points at closing the window instead. Only on
/// confirm does it set `QUITTING`, which is what lets this exit past the
/// `ExitRequested` guard.
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
                return;
            }
            QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
            desktop::stop_service();
            quit_app.exit(0);
        });
}

/// Fully uninstall Lucidos from the GUI, so a non-developer never needs a
/// terminal. Two native confirms, because `tauri_plugin_dialog` caps at two
/// buttons and the keep-versus-delete choice needs its own dialog.
///
/// The plugin cannot make a non-default button the native default, so the
/// affirmative one is highlighted and the copy carries the warning. Success
/// dismisses into a HARD process exit, never `app.exit`, whose on-exit handlers
/// would let the window-state plugin re-create the data dir just deleted. An
/// error keeps the app running so the user can retry.
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
                return;
            }
            // The cancel slot ("Keep My Data") is the SAFE choice. Escape here
            // keeps data and still proceeds with the uninstall the user already
            // committed to.
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
                    // Both buttons proceed with the uninstall, and differ only
                    // over the data. Hide the windows BEFORE the destructive
                    // steps tear the service down, or the user stares at a
                    // window whose backend is dying. The result dialogs are
                    // app-level, so they show with no window visible.
                    hide_all_windows(&fate_app);
                    run_uninstall(fate_app.clone(), delete_data);
                });
        });
}

/// Hide every Lucidos client window. Best-effort, since a hide failure must not
/// abort the uninstall. Window messages are proxied to the main event loop, so
/// this is safe from the dialog callback thread.
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
                    // Hard-exit rather than `app.exit(0)`. A normal exit runs
                    // the `RunEvent::Exit` handlers, and the window-state plugin
                    // would re-persist into a freshly re-created app-support
                    // dir, leaving residue after "Delete Everything".
                    .show(|_| std::process::exit(0));
            }
            Err(e) => report_uninstall_error(&app, &e),
        }
    });
}

/// Surface an uninstall failure in a native dialog and the logs. Does NOT exit,
/// so the user can retry. Re-shows the main window, hidden when the uninstall
/// was confirmed, so they are not stranded with no window to retry from.
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

/// What the packaged startup is waiting on, for the pre-gateway boot splash.
///
/// The window paints that splash on the bundled asset scheme, where it can
/// reach no API at all until `desktop::launch` navigates it to the gateway, so
/// this IPC call is its only source of news. The label is composed in Rust
/// (`desktop::startup_label`) rather than assembled here or in the page, so the
/// wording has one home and is unit-tested.
#[tauri::command]
fn startup_status(app: tauri::AppHandle) -> String {
    app.state::<desktop::StartupStatus>().label()
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
fn focus_calling_window(app: tauri::AppHandle, window: tauri::Window) {
    // Restore `Regular` BEFORE showing: the AppKit `Accessory` to `Regular`
    // transition otherwise leaves the app behind other apps with an unclickable
    // menu bar.
    set_menu_bar_only(&app, false);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    activate_app_frontmost();
    // `set_focus()` also fires `WindowEvent::Focused(true)`, but emit explicitly
    // so the reshow is deterministic regardless of event timing.
    emit_window_active(&app, window.label(), true);
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
/// `link` is the service-worker message shape plus the calling page's
/// `workspace`, its gateway slug. That slug routes a tap back to the workspace
/// that RAISED it, not whichever one happens to be loaded. It also composes the
/// UN request identifier. On tap the delegate emits
/// `native-notification-tapped`, and the page routes it through the same
/// `dispatchDeepLink` the web-push tap uses.
///
/// No-op in `tauri dev` and off macOS. See `notifications.rs` and
/// `system-knowhow/notifications.md` §4.
#[tauri::command]
fn show_native_notification(title: String, body: String, link: serde_json::Value) {
    notifications::show(&title, &body, link);
}

/// Remove an already-delivered native banner, the cross-device dismiss
/// counterpart of [`show_native_notification`]. `Some(id)` removes one banner,
/// and `None` removes every delivered banner `workspace` raised, leaving the
/// other workspaces alone.
///
/// `workspace` is the same gateway slug [`show_native_notification`] stamped
/// into the link, and BOTH arms need it: `Some(id)` rebuilds the composite
/// request identifier, and `None` scopes the sweep. The stashed deep link goes
/// with the banner, so a tap on a removed one cannot route. No-op in
/// `tauri dev` and off macOS.
#[tauri::command]
fn dismiss_native_notification(workspace: Option<String>, id: Option<String>) {
    notifications::dismiss(workspace, id);
}

/// Wake the unread-indicator loop for an immediate recompute, so the macOS
/// count updates the instant a notification is read rather than on the next
/// poll. Best-effort: a send failure with no receiver is ignored. The recompute
/// reads the gateway's fresh `unread-total` aggregate.
#[tauri::command]
fn nudge_dock_badge(state: tauri::State<'_, DockBadgeNudge>) {
    let _ = state.0.lock().unwrap().send(());
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
fn get_native_window_active(window: tauri::Window) -> bool {
    let focused = window.is_focused().unwrap_or(false);
    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    focused && visible && !minimized
}

/// Drain the deep links from native-banner taps the page may not have been
/// listening for. The drain is atomic, so each tap routes exactly once.
///
/// `workspace` is the calling page's gateway slug, and it scopes the drain: a
/// page takes only the taps raised by the workspace it is serving, plus
/// unattributable ones. Without it a window on one workspace could swallow
/// another workspace's tap.
#[tauri::command]
fn take_pending_native_taps(workspace: Option<String>) -> Vec<serde_json::Value> {
    notifications::take_pending_taps(workspace.as_deref())
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
fn emit_window_active(app: &tauri::AppHandle, label: &str, active: bool) {
    let _ = app.emit_to(label, "native-window-active", active);
}

/// True while the packaged macOS client is running menu-bar-only: no visible app
/// window, the `Accessory` activation policy, and absent from the Dock and
/// Cmd+Tab. [`apply_unread_indicator`] reads it to tell whether there is a Dock
/// tile to badge, since the tray carries the count either way. Starts `false`,
/// so an ordinary launch is a normal `Regular` Dock app.
static MENU_BAR_ONLY: AtomicBool = AtomicBool::new(false);

/// The most recent aggregate unread total. An activation-policy transition
/// re-applies the SAME count to the Dock tile that just appeared or vanished,
/// rather than waiting for the next badge-loop poll.
static LAST_UNREAD: AtomicU64 = AtomicU64::new(0);

/// The client should be menu-bar-only exactly when no app window is visible.
fn should_be_menu_bar_only(visible_app_windows: usize) -> bool {
    visible_app_windows == 0
}

/// Should a macOS *reopen* put a window back on screen? Only when the user can
/// see none, which is the one thing
/// `applicationShouldHandleReopen:hasVisibleWindows:` exists to answer.
///
/// **With a window already up, a reopen must change nothing**, and that is a fix
/// rather than a tidy-up. macOS raises the event for a Dock-icon click, a Finder
/// re-open, and an activation driven by a notification tap. The Dock activates
/// the app by itself either way. Answering any of them by fronting `main` raises
/// whatever workspace `main` is pointed at, over the window the user was on.
/// That is the rule [`focus_calling_window`] already states, and a native banner
/// tap is where it bit: `route_native_tap` picks the window on the workspace
/// that raised the banner, and the reopen then overruled it.
///
/// Compiled off macOS only for its test: `RunEvent::Reopen` is macOS-only.
#[cfg(any(target_os = "macos", test))]
fn reopen_shows_a_window(visible_app_windows: usize) -> bool {
    visible_app_windows == 0
}

/// Should this launch put its window on screen?
///
/// `main` is declared `"visible": false` and shown from here instead, so the ONE
/// launch that wants no window never has to flash one first. That launch is the
/// login agent's, which passes [`desktop::LOGIN_FLAG`] and comes up
/// menu-bar-only.
///
/// In dev the answer is always yes. `install_tray` is skipped there, so a hidden
/// dev window would have nothing to reopen it.
fn should_show_window_at_startup(args: &[std::ffi::OsString], is_dev: bool) -> bool {
    is_dev
        || !args
            .iter()
            .any(|a| a == std::ffi::OsStr::new(desktop::LOGIN_FLAG))
}

/// How long `setup` waits for [`window_ready_to_show`] before putting the window
/// on screen unpainted.
///
/// A healthy launch signals in well under a second, since the first document is
/// the bundled boot splash and needs no network. So this elapses only when the
/// frontend is genuinely not coming, and the cost is a launch that feels slow
/// rather than one that fails. Well below the watchdog's first check too, so the
/// window is up long before recovery looks at it.
const STARTUP_SHOW_FALLBACK: std::time::Duration = std::time::Duration::from_secs(3);

/// The one-shot gate on the deferred startup show.
///
/// `main` stays hidden until the frontend says it has something to paint. A cold
/// launch then never shows a window holding nothing but the window-layer tint.
/// Two racers end that wait, and both are required: the frontend's
/// [`window_ready_to_show`], and the [`STARTUP_SHOW_FALLBACK`] timer covering a
/// frontend that never signals. Whichever arrives first shows the window and the
/// other is a no-op, so a user who has since dismissed it does not get it back.
///
/// `wanted` is the launch decision and it gates BOTH racers. It starts false, so
/// a missing `arm` shows no window rather than defeating the login-start
/// suppression. Being menu-bar-only gates them too, which covers a dismissal
/// that lands inside the wait (see [`StartupShow::claim`]).
struct StartupShow {
    wanted: AtomicBool,
    shown: AtomicBool,
}

impl StartupShow {
    const fn new() -> Self {
        Self {
            wanted: AtomicBool::new(false),
            shown: AtomicBool::new(false),
        }
    }

    /// Record whether this launch gets a window at all. Called once, from `setup`.
    fn arm(&self, wanted: bool) {
        self.wanted
            .store(wanted, std::sync::atomic::Ordering::SeqCst);
    }

    /// True for exactly one caller of an armed gate, false for every caller of
    /// an unarmed one. The racers are indistinguishable here: first one wins,
    /// which is what makes both arrival orders safe.
    ///
    /// `menu_bar_only` stands both of them down, and is read BEFORE the one-shot
    /// is consumed so a legitimate later show can still have it. The app menu is
    /// live from the moment the event loop starts, so a user can drop the client
    /// to `Accessory` inside the wait. Showing then would put a window on screen
    /// with no Dock tile, behind the other apps and with an unclickable menu bar.
    fn claim(&self, menu_bar_only: bool) -> bool {
        self.wanted.load(std::sync::atomic::Ordering::SeqCst)
            && !menu_bar_only
            && !self.shown.swap(true, std::sync::atomic::Ordering::SeqCst)
    }
}

static STARTUP_SHOW: StartupShow = StartupShow::new();

/// Put the deferred startup window on screen, at most once. Returns whether this
/// call was the one that did it, so the fallback timer can say that the frontend
/// never signalled without the frontend's own path logging anything.
fn show_startup_window(app: &tauri::AppHandle) -> bool {
    if !STARTUP_SHOW.claim(MENU_BAR_ONLY.load(std::sync::atomic::Ordering::SeqCst)) {
        return false;
    }
    match app.get_webview_window("main") {
        Some(win) => {
            if let Err(e) = win.show() {
                eprintln!("[Tauri] Failed to show the main window: {e}");
            }
        }
        None => eprintln!("[Tauri] No main window to show at startup"),
    }
    true
}

/// The frontend's "theme resolved, about to paint" signal (`windowReadyToShow`
/// in `utils/tauri.ts`). Ends the deferred startup wait, so the first frame the
/// user sees is a painted page rather than a bare window.
///
/// Scoped to `main`: it is the only window that starts hidden. A New-Window child
/// is built visible and its frontend signals too, which must not consume the
/// one-shot `main` is still waiting on.
#[tauri::command]
fn window_ready_to_show(window: tauri::Window) {
    if window.label() != "main" {
        return;
    }
    show_startup_window(window.app_handle());
}

/// Which surfaces show the unread `count`, as `(dock_badge_label, tray_title)`.
/// The tray title ALWAYS carries the count. The Dock badge carries it while the
/// client is a `Regular` Dock app, and stays `None` while menu-bar-only, where
/// there is no tile to badge. Zero clears both, and a count over 99 collapses
/// to "99+".
///
/// **The two halves clear differently, which is why their types differ.** The
/// Dock badge is an `Option`, because `setBadgeLabel(None)` removes the bubble.
/// The tray title is a `String` whose EMPTY value means no count. `tray-icon`'s
/// macOS `set_title` ignores a `None` outright. A cleared title must therefore
/// be WRITTEN empty, or the menu bar keeps the last number it was given.
fn unread_targets(menu_bar_only: bool, count: u64) -> (Option<String>, String) {
    if count == 0 {
        return (None, String::new());
    }
    let label = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    let dock = if menu_bar_only {
        None
    } else {
        Some(label.clone())
    };
    (dock, label)
}

/// Show the aggregate unread `count` on every surface that exists right now.
/// Stores it in [`LAST_UNREAD`] so a later activation transition can re-apply it
/// to the Dock tile that just appeared or vanished.
pub(crate) fn apply_unread_indicator(app: &tauri::AppHandle, count: u64) {
    LAST_UNREAD.store(count, std::sync::atomic::Ordering::SeqCst);
    let menu_bar_only = MENU_BAR_ONLY.load(std::sync::atomic::Ordering::SeqCst);
    let (dock, tray) = unread_targets(menu_bar_only, count);
    notifications::set_dock_badge(dock);
    notifications::set_tray_title(app, &tray);
}

/// Switch the client between menu-bar-only and a normal Dock app. On macOS this
/// sets the NSApplication activation policy; elsewhere only the flag moves. Then
/// re-applies the unread indicator, so the Dock tile that just appeared or
/// vanished agrees with the tray. See
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

/// How many top-level app windows the user can actually see. `excluding` skips a
/// window that is on its way out but might still be listed. An unreadable
/// visibility counts as hidden: that keeps the tray and the reopen path from
/// leaving the client with nothing on screen.
fn visible_app_windows(app: &tauri::AppHandle, excluding: Option<&str>) -> usize {
    app.webview_windows()
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
fn enter_menu_bar_only_if_no_windows(app: &tauri::AppHandle, excluding: Option<&str>) {
    if should_be_menu_bar_only(visible_app_windows(app, excluding)) {
        set_menu_bar_only(app, true);
    }
}

/// Activate the app frontmost on macOS. No-op elsewhere.
fn activate_app_frontmost() {
    #[cfg(target_os = "macos")]
    notifications::activate_app();
}

/// Close the whole client to the menu-bar tray. Destroys secondary windows and
/// HIDES `main`, so reopen is instant and preserves page state, then drops to
/// menu-bar-only. The launchd services are untouched, and the only full teardown
/// is [`quit_lucidos`].
///
/// Packaged only. Dev has no always-on service and no tray, so hiding and going
/// `Accessory` would strand the window with no way to reopen it. Dev therefore
/// closes the windows instead, matching the default close-quits behavior.
fn close_all_to_tray(app: &tauri::AppHandle) {
    if tauri::is_dev() {
        for (label, window) in app.webview_windows() {
            if is_app_window(&label) {
                let _ = window.close();
            }
        }
        return;
    }
    // The plugin's exit-time write never runs, because we hide rather than
    // exit, so this is the moment to remember size and position.
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

/// Show and focus the main window, recreating it if it was destroyed. Backs the
/// tray's "Open Lucidos" item, the Dock click, and a native-notification tap, so
/// a window hidden on close can always be brought back.
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if app.get_webview_window(MAIN_WINDOW_LABEL).is_some() {
        front_window(app, MAIN_WINDOW_LABEL);
        return;
    }
    // Gone, so build a replacement. Leaving menu-bar-only first for the reason
    // `front_window` does: `Accessory` cannot front the new window either.
    set_menu_bar_only(app, false);
    if let Err(e) = open_new_window(app) {
        eprintln!("[Tauri] Failed to open window: {e}");
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
    set_menu_bar_only(app, false);
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        activate_app_frontmost();
        emit_window_active(app, label, true);
    }
}

/// Bring forward (or create) the window a native banner tap belongs in, given
/// the workspace that RAISED the banner.
///
/// One packaged process fronts the gateway and each app window can sit on its
/// own workspace (ADR 0014), so "the window that is frontmost" and "the
/// workspace the tap came from" are unrelated. The decision lives here because
/// only this process can see every window, read what each is pointed at, and
/// open one.
///
/// Returns the label of an already-loaded window to send the warm
/// `native-notification-tapped` wake to, or `None` when the target is a page
/// that is about to load: a fresh page runs the startup drain itself, and an
/// `emit` into a webview mid-navigation is dropped.
///
/// The caller must have stashed the tap BEFORE calling this. Showing or focusing
/// a window fires that page's `focus` / `visibilitychange` drains, and a drain
/// that runs first finds nothing.
#[cfg(target_os = "macos")]
pub(crate) fn route_native_tap(app: &tauri::AppHandle, owner: Option<&str>) -> Option<String> {
    let windows: Vec<(String, String)> = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| is_app_window(label))
        .map(|(label, window)| {
            let url = window.url().map(|u| u.to_string()).unwrap_or_else(|e| {
                // An unreadable URL reads as "not navigated", which sends the
                // tap down the boot path. Say so, or it is silently stranded.
                eprintln!("[Tauri] Could not read the URL of window {label}: {e}");
                String::new()
            });
            (label, url)
        })
        .collect();

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
            match (app.get_webview_window(&label), url.parse::<tauri::Url>()) {
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
            match url.parse::<tauri::Url>() {
                Ok(parsed) => {
                    set_menu_bar_only(app, false);
                    if let Err(e) = open_app_window(app, WebviewUrl::External(parsed)) {
                        eprintln!("[Tauri] Failed to open a window for {url}: {e}");
                    }
                    activate_app_frontmost();
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
            app.get_webview_window(MAIN_WINDOW_LABEL)
                .map(|_| MAIN_WINDOW_LABEL.to_string())
        }
    }
}

/// Install the macOS menu-bar status item, in packaged builds only. It keeps the
/// client resident after the window is dismissed, and hosts the only full
/// teardown.
fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    // macOS status items are TEMPLATE IMAGES: a transparent-background
    // silhouette the system renders monochrome and inverts per bar. The
    // full-colour app icon would sit there as a lone blue square. So this is a
    // dedicated silhouette flagged `icon_as_template`, whose alpha is all macOS
    // reads. Regenerate it with `icons/gen-tray-template.py`.
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
    // Created here so both ends are in scope: `nudge_dock_badge` holds the
    // sender as managed state, and `desktop::launch` takes the receiver.
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
        .manage(desktop::StartupStatus::default())
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
            startup_status,
            restart_app,
            restart_service,
            quit_lucidos,
            uninstall_lucidos,
            open_url_external,
            save_to_downloads,
            show_native_notification,
            dismiss_native_notification,
            focus_calling_window,
            nudge_dock_badge,
            get_native_window_active,
            take_pending_native_taps,
            updater::check_app_update,
            updater::install_app_update_and_restart,
            updater::cancel_app_update,
            set_titlebar_color,
            set_traffic_light_offset,
            window_ready_to_show,
            start_window_drag,
            toggle_window_maximize,
            open_workspace_window,
            mobile::get_connect_info,
            mobile::tailscale_up,
            mobile::tailscale_serve,
            mobile::cancel_tailscale_serve,
            device_id_store::get_or_create_device_id,
            device_id_store::previous_device_id,
            device_id_store::remember_device_id,
            pairing::mint_pairing_code,
        ])
        .on_window_event(|window, event| {
            let app = window.app_handle();
            match event {
                // Packaged: closing a window must NOT quit the client or stop
                // the always-on service, so the main window is hidden rather
                // than closed. Secondary windows close normally, and dev keeps
                // the default close-quits behavior.
                //
                // Geometry is flushed synchronously here on every platform. The
                // plugin's own disk write is unreachable in the packaged client,
                // and this close is when the user expects it remembered.
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    if let Err(e) = app.save_window_state(window_state_flags()) {
                        eprintln!("[Tauri] Failed to persist window state on close: {e}");
                    }
                    if !tauri::is_dev() && window.label() == "main" {
                        // Keep the client resident: HIDE, for a fast reopen with
                        // page state preserved, then drop to the tray if this was
                        // the last visible window.
                        api.prevent_close();
                        let _ = window.hide();
                        // The WKWebView cannot report an `orderOut:`, and
                        // `Focused(false)` may not fire on one. This is the
                        // load-bearing signal that the page is inactive. It is
                        // what lets the engine send the OS banner rather than a
                        // suppressed in-app toast.
                        emit_window_active(app, window.label(), false);
                        enter_menu_bar_only_if_no_windows(app, Some(window.label()));
                    }
                }
                // A secondary window closed. Drop its traffic-light resize
                // observer, then re-evaluate the tray. The tray half is
                // packaged-only; the observer half is not, because a leaked
                // registration is keyed on the dead window's address in both
                // builds. `main` never reaches here: its close is prevented.
                tauri::WindowEvent::Destroyed if is_app_window(window.label()) => {
                    traffic_lights::unwatch(window.label());
                    if !tauri::is_dev() {
                        enter_menu_bar_only_if_no_windows(app, Some(window.label()));
                    }
                }
                // Bridge native focus so `isPageActive()` reads the AppKit state
                // rather than the flaky WKWebView `hasFocus()`. The trayed case
                // is covered by the explicit emit in `CloseRequested` above.
                tauri::WindowEvent::Focused(focused) if is_app_window(window.label()) => {
                    emit_window_active(app, window.label(), *focused);
                }
                // Arm the debounced background flush. The plugin keeps its own
                // in-memory cache from these same events, and the disk write is
                // what makes the geometry survive a relaunch.
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
                    if is_app_window(window.label()) =>
                {
                    // A RESIZE puts the macOS window buttons back where AppKit
                    // wants them, discarding our placement, so re-apply it.
                    //
                    // This is the LATE net, not the one that keeps a live drag
                    // steady: tao queues the event, so it lands a run-loop turn
                    // after AppKit's reset is already on screen, which is what
                    // `traffic_lights::watch_resizes` fixes. It stays for the
                    // moment the notification does not cover, tao's synthetic
                    // resize on leaving fullscreen, where late is right.
                    // Idempotent, so the overlap costs nothing. Not on `Moved`:
                    // the placement is a function of the window's HEIGHT.
                    if matches!(event, tauri::WindowEvent::Resized(_)) {
                        traffic_lights::place(window);
                    }
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
            // Per-window close is the standard Close Window item, handled in
            // `on_window_event` above rather than here.
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
                // One early eval, so the reclaimed-band CSS vars are set before
                // the first paint.
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
            // The plugin has already written the saved rect onto `main`, and
            // nothing has shown the window yet. This is therefore the one moment
            // a corrupt or now-impossible rect can be corrected off screen. A
            // healthy rect makes it a no-op. See `window_restore`.
            window_restore::clamp_restored_geometry(app.handle(), MAIN_WINDOW_LABEL);

            // Best-effort: a menu build failure must not block app startup.
            if let Err(e) = install_app_menu(app) {
                eprintln!("[Tauri] Failed to install app menu: {e}");
            }

            // The color the frontend last asked for, so a light-theme user does
            // not launch into the dark-theme blue. `applyTheme` refines it once
            // this launch's theme is known.
            if let Some(color) = pre_paint_title_bar_color(app.handle()) {
                paint_title_bars(app.handle(), color);
            }

            // The other half of the band: the traffic lights on the bar the
            // frontend last measured. Load first, then place, so the very first
            // window is centred for the user's UI scale.
            traffic_lights::load_persisted(app.handle());
            traffic_lights::place_all(app.handle());

            // No-op in dev, where there is no always-on service to represent.
            if !tauri::is_dev() {
                if let Err(e) = install_tray(app) {
                    eprintln!("[Tauri] Failed to install tray icon: {e}");
                }
            }

            // Decide whether this launch gets a window. Ordered AFTER the tray
            // on purpose: going `Accessory` with neither a window nor a tray
            // item would leave the client with no surface at all.
            //
            // The DECISION is taken here, but showing waits for the frontend's
            // `window_ready_to_show`, with the timer below as the safety net.
            // Showing now would put an unpainted window on screen for as long as
            // the webview takes to load. A login start arms the gate `false`,
            // which stands both racers down.
            let launch_args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
            let show_at_startup = should_show_window_at_startup(&launch_args, tauri::is_dev());
            STARTUP_SHOW.arm(show_at_startup);
            if show_at_startup {
                // The safety net. Without it, a frontend that never signals (a
                // webview crash, a JS exception before the signal, a bundle that
                // does not load) leaves a client with a tray icon and no way to
                // discover it has no window.
                let show_handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(STARTUP_SHOW_FALLBACK);
                    if show_startup_window(&show_handle) {
                        eprintln!(
                            "[Tauri] No ready-to-show signal within {STARTUP_SHOW_FALLBACK:?}: \
                             showing the window anyway. The page may have failed to load, so \
                             check the engine log for [Client/ipc] lines."
                        );
                    }
                });
            } else {
                eprintln!("[Tauri] Started at login: coming up menu-bar-only, no window");
                set_menu_bar_only(app.handle(), true);
            }

            // WKWebView crash recovery. A heartbeat that stops arriving means
            // the content process probably crashed, so reload to recover. A
            // reload that brings back no heartbeat fixed nothing, so the
            // interval backs off (see `ReloadWatchdog`) and says so: a silent
            // reload loop hides the real fault for months.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                // Let the webview load before the watchdog starts.
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

            // Debounced window-geometry flush, shortly after the user stops
            // dragging or resizing. See `GeometrySaver` for why the plugin's own
            // exit-time write is not enough. The save is marshalled onto the
            // main thread: saving from this worker would deadlock the UI.
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

            // Register the UserNotifications delegate and request notification
            // authorization. No-op in dev. See `notifications.rs`.
            notifications::setup(app.handle());

            // Packaged: boot the bundled Postgres and engine, then point the
            // window at them. No-op in development.
            desktop::launch(app.handle(), dock_badge_nudge_rx);
            // Update detection surfaces INSIDE the workspace UI, as an in-app
            // toast (see `updater.rs`). There is no native launch dialog.
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // The engine and Postgres run as a launchd service, independent of
            // the client. Packaged: keep the client process alive when the last
            // window is dismissed, so it can host the menu-bar item. A Dock
            // click on the windowless client re-shows one. `quit_lucidos` sets
            // `QUITTING` so its own exit passes the guard.
            match event {
                tauri::RunEvent::ExitRequested { api, .. } => {
                    if !tauri::is_dev() && !QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
                        api.prevent_exit();
                    }
                }
                // A reopen brings the client back only when nothing is on
                // screen. See `reopen_shows_a_window` for why a window already
                // up is left exactly where the user put it.
                //
                // The event's own `has_visible_windows` is deliberately unused:
                // it is AppKit's count over every `NSWindow` this process owns,
                // and the menu-bar status item owns one. Trusting it would read
                // a trayed client as "has windows" and never reopen. We count
                // APP windows instead, the same way the tray path does.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    if !tauri::is_dev()
                        && reopen_shows_a_window(visible_app_windows(app_handle, None))
                    {
                        show_main_window(app_handle);
                    }
                }
                _ => {}
            }
        });
}

/// Build and install the app menu.
///
/// DERIVE from Tauri's OS-default menu rather than building every submenu by
/// hand. This is load-bearing on macOS: the system loads the standard
/// text-editing key bindings for a WKWebView only when the app exposes a
/// COMPLETE native app menu. Without the default App menu's predefined items,
/// `interpretKeyEvents:` finds no command for the arrow keys. It falls back to
/// `insertText:`, typing raw NSFunctionKey characters into the focused textarea.
///
/// The frontend refuses such a character before it reaches a field
/// (`utils/noFunctionKeyText.ts`). It has to: an arrow press at the END of the
/// text falls through to `insertText:` even with this menu in place. So
/// breaking the menu no longer types squares: watch instead for arrow keys
/// doing nothing at all.
///
/// macOS then grafts on the service-aware items. Cmd+Q maps to "Close to Menu
/// Bar", and the full teardown is the separate, unshortcutted "Quit and Stop
/// Background Service". Other platforms keep the default menu unchanged.
fn install_app_menu(app: &tauri::App) -> tauri::Result<()> {
    let menu = Menu::default(app.handle())?;

    #[cfg(target_os = "macos")]
    {
        use tauri::menu::MenuItemKind;

        // Default macOS submenu order is [App, File, Edit, View, Window, Help].
        let items = menu.items()?;

        if let Some(MenuItemKind::Submenu(app_menu)) = items.first() {
            // Drop the default Quit, the last item, so Cmd+Q is free for "Close
            // to Menu Bar". Removing only Quit leaves About, Services and Hide
            // intact, so the menu is still recognized as the Apple menu, which
            // is the arrow-key fix above. The item before Quit is a separator,
            // so the grafted items start straight at Uninstall.
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

    /// The injected script is a string nothing else parses. A typo in it fails
    /// silently in the webview, and the only symptom is a header laid out like
    /// the web build. The attribute is the gate every `[data-titlebar-overlay]`
    /// rule hangs off, and the lights x is what makes
    /// `--titlebar-lights-reserve` arithmetic rather than an estimate.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_titlebar_script_sets_the_inset_the_lights_x_and_the_overlay_marker() {
        let script = titlebar_inset_script();
        assert!(script.contains("--titlebar-inset"), "{script}");
        assert!(script.contains("28px"), "{script}");
        assert!(script.contains("data-titlebar-overlay"), "{script}");
        // The stamped x has to BE the x the placement uses, rendered as a CSS
        // length. Built from the constant, so this catches the rendering rather
        // than the value: a stray `10` with no unit is not a length, and the
        // reserve's calc would be invalid.
        assert!(script.contains("--titlebar-lights-x"), "{script}");
        assert!(
            script.contains(&format!("'{}px'", traffic_lights::LIGHTS_X_PX)),
            "{script}"
        );
        // All three statements live inside the one `if(document.documentElement)`
        // guard, so the braces have to balance or the later ones never run.
        assert_eq!(
            script.matches('{').count(),
            script.matches('}').count(),
            "unbalanced braces: {script}"
        );
    }

    /// Off macOS there is no native title bar, so the script must stay empty.
    /// Stamping the marker elsewhere would move that header's leading control
    /// into a band that does not exist.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn the_titlebar_script_is_empty_off_macos() {
        assert_eq!(titlebar_inset_script(), "");
    }

    /// The stable gateway URL, standing in for what the command passes as its
    /// fallback. Any value works: what the tests pin is when it is reached for.
    const FALLBACK: &str = "http://localhost:3210";

    /// The second window lands on the origin the CALLING window is already on,
    /// which is what makes this match the web path's origin-relative `/<slug>/`.
    #[test]
    fn a_new_workspace_window_takes_the_callers_own_origin() {
        assert_eq!(
            workspace_window_url(Some("http://localhost:3210/dev/?pick"), "work", FALLBACK),
            Some("http://localhost:3210/work/".to_string())
        );
        // Reached over a tailnet address, so the second window goes there too:
        // sending it to loopback would open a window the user cannot use.
        assert_eq!(
            workspace_window_url(Some("https://box.tailnet.ts.net/dev/"), "work", FALLBACK),
            Some("https://box.tailnet.ts.net/work/".to_string())
        );
        // A dev window on the vite port stays on it.
        assert_eq!(
            workspace_window_url(Some("http://localhost:5173/~/"), "work", FALLBACK),
            Some("http://localhost:5173/work/".to_string())
        );
    }

    /// A caller on no http(s) origin at all: the bundled asset scheme, before
    /// `desktop::launch` has navigated the window. `tauri::Url::origin()` would
    /// answer the literal "null" there, which is why the fallback exists.
    #[test]
    fn a_caller_on_no_http_origin_falls_back_to_the_gateway_url() {
        for caller in [None, Some("tauri://localhost"), Some("")] {
            assert_eq!(
                workspace_window_url(caller, "work", FALLBACK),
                Some("http://localhost:3210/work/".to_string()),
                "caller {caller:?}"
            );
        }
    }

    /// The page supplies the slug. So this refusal is the whole gate on what can
    /// load in a window carrying the `window-*` IPC grant (ADR 0028).
    #[test]
    fn a_workspace_that_is_not_a_slug_opens_nothing() {
        for bad in [
            "",
            "..",
            "../../etc",
            "~",
            "work/../dev",
            "Work",
            "work space",
            "http://evil.example.com",
            "-work",
            "work-",
        ] {
            assert_eq!(
                workspace_window_url(Some("http://localhost:3210/dev/"), bad, FALLBACK),
                None,
                "{bad:?} was accepted"
            );
        }
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
    fn a_reopen_only_shows_a_window_when_none_is_on_screen() {
        // Trayed / windowless: a Dock click is the way back, so show one.
        assert!(reopen_shows_a_window(0));
        // The bug: with a window up, fronting `main` raised whatever workspace
        // it was on over the one the user was looking at. A native banner tap
        // reaches here through the reopen macOS raises when it activates the
        // app, right after `route_native_tap` chose the raising workspace's
        // window. Leave the order alone.
        assert!(!reopen_shows_a_window(1));
        assert!(!reopen_shows_a_window(2));
    }

    #[test]
    fn the_declared_main_window_starts_hidden() {
        // WE put the window on screen, which is only a choice while the config
        // keeps it hidden. Make this window `"visible": true` and every login
        // start flashes a window before hiding it, while every other launch
        // shows an unpainted one.
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            conf["app"]["windows"][0]["visible"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn only_a_packaged_login_start_comes_up_without_a_window() {
        let os = |s: &str| std::ffi::OsString::from(s);
        let login = [os(desktop::LOGIN_FLAG)];

        // The login agent's launch: menu bar only, no window, ever. This is the
        // one launch that leaves `main` hidden, rather than flashing a window
        // at every boot.
        assert!(!should_show_window_at_startup(&login, false));
        // Alongside other arguments it still counts.
        assert!(!should_show_window_at_startup(
            &[os("--other"), os(desktop::LOGIN_FLAG)],
            false
        ));

        // Every human launch shows the window: a double-click, and the updater's
        // relaunch, which forwards whatever argv it was started with.
        assert!(should_show_window_at_startup(&[], false));
        assert!(should_show_window_at_startup(&[os("--other")], false));
        // Not a prefix match: `--login-something` is not the flag.
        assert!(should_show_window_at_startup(&[os("--login-shell")], false));

        // In dev the flag is inert. There is no tray there (`install_tray` is
        // skipped), so a hidden dev window would have nothing to reopen it.
        assert!(should_show_window_at_startup(&login, true));
    }

    #[test]
    fn the_ready_signal_shows_the_window_and_a_later_timeout_is_a_no_op() {
        // The healthy launch: the frontend reports it is about to paint, and the
        // fallback timer fires three seconds later into an already-shown window.
        let gate = StartupShow::new();
        gate.arm(true);
        assert!(gate.claim(false), "the ready signal shows the window");
        assert!(
            !gate.claim(false),
            "the fallback timer must not show it again"
        );
    }

    #[test]
    fn the_timeout_shows_the_window_and_a_later_ready_signal_is_a_no_op() {
        // The frontend was slow rather than dead, so the signal lands after the
        // safety net has already shown the window. Re-showing here is not merely
        // redundant: by then the user may have dismissed it to the menu bar.
        let gate = StartupShow::new();
        gate.arm(true);
        assert!(gate.claim(false), "the fallback timer shows the window");
        assert!(
            !gate.claim(false),
            "the late ready signal must not show it again"
        );
    }

    #[test]
    fn a_login_start_shows_the_window_from_neither_racer() {
        // `should_show_window_at_startup` said no, so the decision holds against
        // BOTH paths: a login start is menu-bar-only however the frontend behaves.
        let gate = StartupShow::new();
        gate.arm(false);
        assert!(!gate.claim(false));
        assert!(!gate.claim(false));
    }

    #[test]
    fn a_dismissal_inside_the_wait_stands_both_racers_down() {
        // Cmd+Q ("Close to Menu Bar") in the second before the window appears
        // drops the client to `Accessory` with nothing visible. Showing then
        // would put a window on screen with no Dock tile behind every other app.
        let gate = StartupShow::new();
        gate.arm(true);
        assert!(!gate.claim(true), "the ready signal must stand down");
        assert!(!gate.claim(true), "and so must the fallback timer");
        // The one-shot was NOT consumed while standing down, so reopening from
        // the tray (which restores `Regular`) leaves a working gate rather than a
        // spent one.
        assert!(gate.claim(false));
    }

    #[test]
    fn the_startup_show_gate_admits_exactly_one_concurrent_racer() {
        // The two racers really do run on different threads (the IPC handler and
        // the fallback timer), so the one-shot has to hold under a genuine race,
        // not just under sequential calls.
        let gate = StartupShow::new();
        gate.arm(true);
        let winners = std::thread::scope(|s| {
            let racers: Vec<_> = (0..8).map(|_| s.spawn(|| gate.claim(false))).collect();
            racers
                .into_iter()
                .map(|r| r.join().expect("racer panicked"))
                .filter(|won| *won)
                .count()
        });
        assert_eq!(winners, 1, "exactly one racer may show the window");
    }

    #[test]
    fn the_startup_color_prefers_the_value_the_frontend_last_asked_for() {
        // The light-theme header blue, remembered from the last session, so a
        // cold launch is not dark blue correcting itself.
        assert_eq!(title_bar_color_or_default(Some("#1a6fd0")), "#1a6fd0");
        // Every shape the color parser accepts, since the file holds whatever
        // the frontend sent.
        assert_eq!(title_bar_color_or_default(Some("#fff")), "#fff");
        assert_eq!(title_bar_color_or_default(Some("#15549eff")), "#15549eff");
    }

    #[test]
    fn the_startup_color_falls_back_when_nothing_usable_was_remembered() {
        // First run, or an unreadable file.
        assert_eq!(title_bar_color_or_default(None), TITLE_BAR_DEFAULT_COLOR);
        // Truncated, hand-edited, or not a color at all. The file is validated
        // on read rather than trusted, and the value only ever reaches a color
        // parse, so garbage costs the default tint and nothing else.
        assert_eq!(
            title_bar_color_or_default(Some("")),
            TITLE_BAR_DEFAULT_COLOR
        );
        assert_eq!(
            title_bar_color_or_default(Some("#15549")),
            TITLE_BAR_DEFAULT_COLOR
        );
        assert_eq!(
            title_bar_color_or_default(Some("rgb(1,2,3)")),
            TITLE_BAR_DEFAULT_COLOR
        );
        assert_eq!(
            title_bar_color_or_default(Some("../../etc/passwd")),
            TITLE_BAR_DEFAULT_COLOR
        );
    }

    #[test]
    fn the_compiled_default_title_bar_color_parses() {
        // `title_bar_color_or_default` hands its answer straight to the parser.
        // A typo in the constant would leave a first run with NO tint rather
        // than the wrong one.
        assert!(TITLE_BAR_DEFAULT_COLOR
            .parse::<tauri::utils::config::Color>()
            .is_ok());
    }

    #[test]
    fn unread_targets_always_titles_the_tray_and_badges_the_dock_only_under_regular() {
        // A window is open, so the count is on BOTH the Dock badge and the tray
        // title. The menu bar carrying it at all times is the point.
        assert_eq!(
            unread_targets(false, 5),
            (Some("5".into()), "5".to_string())
        );
        // Menu-bar only (Accessory): tray title only, because there is no Dock tile.
        assert_eq!(unread_targets(true, 5), (None, "5".to_string()));
        // Over 99 collapses to "99+" on every surface that shows it.
        assert_eq!(
            unread_targets(false, 100),
            (Some("99+".into()), "99+".to_string())
        );
        assert_eq!(
            unread_targets(false, 150),
            (Some("99+".into()), "99+".to_string())
        );
        assert_eq!(unread_targets(true, 100), (None, "99+".to_string()));
        // Boundary: exactly 99 is shown as-is, on both surfaces.
        assert_eq!(
            unread_targets(false, 99),
            (Some("99".into()), "99".to_string())
        );
        assert_eq!(unread_targets(true, 99), (None, "99".to_string()));
    }

    #[test]
    fn a_cleared_tray_title_is_an_empty_string_the_tray_will_actually_write() {
        // Zero has to reach `set_tray_title` as a value it WRITES, because
        // `tray-icon`'s macOS backend drops a `None` and leaves the last count
        // on screen. An empty string takes the same path a real count does, so
        // the item blanks.
        for menu_bar_only in [false, true] {
            let (dock, tray) = unread_targets(menu_bar_only, 0);
            assert_eq!(dock, None, "the Dock badge clears with None, which works");
            assert_eq!(
                tray, "",
                "the tray clears by being WRITTEN empty, not skipped"
            );
        }
        // And the non-zero case must not be empty, or clearing would be
        // indistinguishable from showing a count.
        assert_ne!(unread_targets(false, 1).1, "");
    }

    #[test]
    fn safari_ua_carries_the_version_and_webkit_suffix() {
        let ua = safari_ua("18.5");
        // WKWebView's default UA lacks the Version and Safari suffix, so ours
        // must carry both, plus the AppleWebKit token.
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
        // Strictly past the timeout: reload. The first one is never futile,
        // since there is no earlier reload for it to have failed to improve on.
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
        // A rejected `invoke`: the page loads and runs, but the count NEVER
        // advances. Without the backoff this reloads once a minute forever and
        // says nothing new.
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
                HEARTBEAT_TIMEOUT,      // 60s, the first attempt
                HEARTBEAT_TIMEOUT * 2,  // 2m
                HEARTBEAT_TIMEOUT * 4,  // 4m
                HEARTBEAT_TIMEOUT * 8,  // 8m
                HEARTBEAT_TIMEOUT * 16, // 16m
                HEARTBEAT_TIMEOUT * 32, // 32m, the ceiling
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
        // Nothing changed, so never flush, however long it has been.
        assert!(!should_persist_geometry(false, Duration::from_secs(10)));
        // Dirty, but the user is still moving or resizing, so wait.
        assert!(!should_persist_geometry(true, Duration::from_millis(0)));
        assert!(!should_persist_geometry(
            true,
            GEOMETRY_SAVE_DEBOUNCE - Duration::from_millis(1)
        ));
        // Dirty and quiet for at least the debounce window, so flush.
        assert!(should_persist_geometry(true, GEOMETRY_SAVE_DEBOUNCE));
        assert!(should_persist_geometry(
            true,
            GEOMETRY_SAVE_DEBOUNCE + Duration::from_millis(1)
        ));
    }

    #[test]
    fn window_state_flags_remembers_geometry_not_visibility() {
        let flags = window_state_flags();
        // Remember where the window is, how big, and on which screen.
        assert!(flags.contains(StateFlags::SIZE));
        assert!(flags.contains(StateFlags::POSITION));
        assert!(flags.contains(StateFlags::MAXIMIZED));
        assert!(flags.contains(StateFlags::FULLSCREEN));
        // But NOT VISIBLE (a flush taken while hidden would restore hidden) and
        // NOT DECORATIONS (toggling it on macOS drops the Overlay title bar).
        assert!(!flags.contains(StateFlags::VISIBLE));
        assert!(!flags.contains(StateFlags::DECORATIONS));
    }

    #[test]
    fn a_plain_file_name_is_kept() {
        assert_eq!(
            leaf_filename("thread-1a2b3c4d-my-thread.json").as_deref(),
            Some("thread-1a2b3c4d-my-thread.json"),
        );
        assert_eq!(
            leaf_filename("  spaced.json  ").as_deref(),
            Some("spaced.json")
        );
    }

    /// The security boundary: every one of these would let `Path::join` write
    /// somewhere the caller never named.
    #[test]
    fn a_name_that_is_not_a_leaf_is_refused() {
        for name in [
            "",
            "   ",
            ".",
            "..",
            "../escape.json",
            "sub/dir.json",
            "sub\\dir.json",
            "/absolute.json",
            ".hidden.json",
        ] {
            assert_eq!(leaf_filename(name), None, "{name:?} must be refused");
        }
    }

    /// A repeat export must not eat the previous one. The counter goes before
    /// the extension so the file stays a readable `.json`.
    #[test]
    fn a_taken_name_gains_a_counter_before_its_extension() {
        let dir = std::env::temp_dir().join(format!("lucidos-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let write = |name: &str, body: &str| write_new_download(&dir, name, body).unwrap();
        assert_eq!(write("t.json", "first"), dir.join("t.json"));
        assert_eq!(write("t.json", "second"), dir.join("t (1).json"));
        assert_eq!(write("t.json", "third"), dir.join("t (2).json"));

        // An extensionless name takes the counter at the end instead.
        assert_eq!(write("bare", "x"), dir.join("bare"));
        assert_eq!(write("bare", "y"), dir.join("bare (1)"));

        // Each write landed where it said, and none clobbered an earlier one.
        assert_eq!(
            std::fs::read_to_string(dir.join("t.json")).unwrap(),
            "first"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("t (1).json")).unwrap(),
            "second"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// Regression coverage for the Tauri ACL, which stands between the packaged
/// window and every one of our IPC commands.
///
/// These drive the REAL resolver over the REAL artifacts: the same
/// `Resolved::resolve` and `RuntimeAuthority::resolve_access` pair
/// `Webview::on_message` consults, the generated schemas, and the runtime
/// capability. So they fail for the same reason the packaged app would.
///
/// What they cannot prove is the origin STRING the OS produces, which tauri
/// derives from the `Origin` header WebKit puts on the `ipc://` request. That
/// the header really is `http://localhost:<port>` rests on code inspection and
/// field evidence, not on these tests.
#[cfg(test)]
mod acl_tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use tauri::ipc::{Origin, RuntimeCapability};
    use tauri::utils::acl::capability::{Capability, CapabilityFile};
    use tauri::utils::acl::manifest::Manifest;
    use tauri::utils::acl::resolved::Resolved;
    use tauri::utils::platform::Target;

    /// The gateway port the tests pin the capability to. Any value works: the
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

    /// Build the authority the packaged client runs with: the static
    /// capabilities, plus the runtime one `desktop::launch` registers before
    /// navigating. Passing `None` leaves the runtime capability out.
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

    /// Plugin commands the frontend depends on, keyed the way the ACL keys
    /// them. `plugin:event|listen` is the load-bearing one: every `listen()` in
    /// the frontend goes through it, so a denial there is silent and total.
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
        // commands go unchecked on LOCAL origins. Its presence is what makes
        // every assertion below meaningful.
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
        // Static capabilities alone, with no remote context, leave every
        // command denied once the window leaves the Tauri asset origin.
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
        // A different local port, a foreign host, and a different scheme must
        // all stay out. Remote content cannot reach custom commands.
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
        // Everything else stays out of reach of previewed content.
        for command in [
            "heartbeat",
            "restart_app",
            "quit_lucidos",
            "uninstall_lucidos",
            "open_url_external",
            "show_native_notification",
            "get_or_create_device_id",
            "previous_device_id",
            "remember_device_id",
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
        // An app ACL manifest switches `has_app_acl_manifest` to true, which
        // starts ACL-checking app commands on LOCAL origins too, dev and the
        // bundled splash included. Without `allow-app-ipc` in the default
        // capability that breaks `tauri dev` and the splash's own heartbeat.
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
        // `generate_handler!` but absent from `permissions/app-ipc.json` is
        // denied on EVERY origin, dev included.
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
        // Same frontend, one reached over http and one off the bundled assets.
        // If the two permission lists drift, a command works in dev and dies in
        // the packaged build, or the reverse.
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
