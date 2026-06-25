use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Mutex;
use std::time::Instant;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::webview::PageLoadEvent;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

mod desktop;
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

/// Tracks the last JS heartbeat timestamp for WKWebView crash recovery.
/// WKWebView's content process can be terminated by macOS under memory pressure,
/// leaving a white screen. The JS side calls `heartbeat` every 15s; if we don't
/// hear from it in 60s, we reload the webview.
struct LastHeartbeat(Mutex<Instant>);

/// How long the JS heartbeat may go silent before the watchdog treats the
/// WKWebView content process as crashed and reloads it. The page heartbeats
/// every 15s, so 60s is four missed beats.
const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// True when the watchdog should reload the webview: the last heartbeat is older
/// than [`HEARTBEAT_TIMEOUT`]. Pure so the threshold is unit-testable without
/// the watchdog thread.
fn heartbeat_expired(elapsed: std::time::Duration) -> bool {
    elapsed > HEARTBEAT_TIMEOUT
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
    let parsed = color.parse().map_err(|e| format!("invalid color {color:?}: {e}"))?;
    paint_title_bars(&app, parsed);
    Ok(())
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
        if let Ok(url) =
            format!("http://localhost:{}", desktop::engine_port()).parse::<tauri::Url>()
        {
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

/// Restart the GUI **client** (re-exec the window shell). This does NOT touch
/// the always-on gateway service — that runs as a launchd LaunchAgent,
/// independent of the window (see `desktop`). To restart the service itself,
/// use `restart_service` (packaged) or `/api/v1/restart` (dev workspace stack).
#[tauri::command]
fn restart_app(app: tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Failed to get current exe: {e}"))?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();

    eprintln!("[Tauri] Restarting app: {:?} {:?}", exe, args);

    app.cleanup_before_exit();

    // On Unix, exec() replaces the process in-place. On other platforms, spawn + exit.
    restart_process(&exe, &args)
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

/// Full teardown: stop the always-on gateway service (`launchctl bootout`), then
/// exit the client. This is the ONLY path that stops the service — closing the
/// window / Cmd+Q merely hide the client (it stays resident in the menu bar), so
/// triggers, scheduled tasks, coding-agent sessions, and push keep running.
/// Reached from the menu-bar "Quit & Stop Background Service" item and the
/// app-menu item of the same name. Sets `QUITTING` so the `ExitRequested` guard
/// lets this exit through. The next app launch re-installs and re-bootstraps the
/// service.
#[tauri::command]
fn quit_lucidos(app: tauri::AppHandle) {
    QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
    desktop::stop_service();
    app.exit(0);
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
    *state.0.lock().unwrap() = Instant::now();
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

/// Show + focus the main window, recreating it if it was destroyed. Backs the
/// menu-bar "Open Lucidos" item and the macOS Dock-click (Reopen), so a window
/// hidden on close can always be brought back.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    } else if let Err(e) = open_new_window(app) {
        eprintln!("[Tauri] Failed to open window: {e}");
    }
}

/// Install the macOS menu-bar (tray) status item — packaged builds only. It keeps
/// the client resident after the window is dismissed: "Open Lucidos" re-shows the
/// window and "Quit & Stop Background Service" is the only full teardown
/// (`quit_lucidos` → `bootout` + exit). The always-on launchd service is
/// unaffected by closing the window.
fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    let Some(icon) = app.default_window_icon().cloned() else {
        eprintln!("[Tauri] No default window icon available; skipping tray icon");
        return Ok(());
    };
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
        "Quit & Stop Background Service",
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
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(PanelWebview(Mutex::new(None)))
        .manage(PanelContentChannel(Mutex::new(None)))
        .manage(LastHeartbeat(Mutex::new(Instant::now())))
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
            updater::check_app_update,
            updater::install_app_update_and_restart,
            set_titlebar_color,
            mobile::get_connect_info,
            mobile::tailscale_up,
            mobile::tailscale_serve,
        ])
        .on_window_event(|window, event| {
            // Packaged: closing a window must NOT quit the client or stop the
            // always-on service. Hide the main window instead of letting it close
            // — the client stays resident in the menu bar (only the tray "Quit &
            // Stop Background Service" tears down). Secondary windows close
            // normally; dev keeps the default close-quits behavior so
            // `tauri-dev.sh` returns when the window is closed.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !tauri::is_dev() && window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .on_menu_event(|app, event| {
            // "Quit & Stop Background Service" (quit_lucidos) is the only teardown;
            // "Close Window" (Cmd+Q) hides the client like the red X; "New Window"
            // opens another client window. Window close (red X / Cmd+W) is handled
            // in on_window_event above, not here.
            if event.id() == "quit_lucidos" {
                quit_lucidos(app.clone());
            } else if event.id() == "close_main_window" {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.close();
                }
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
        .setup(|app| {
            // Install the app menu. The standard edit/window items keep the
            // usual shortcuts; Cmd+Q maps to "Close Window" (hide the client) and
            // the explicit "Quit & Stop Background Service" item drives
            // on_menu_event → quit_lucidos. Best-effort: a menu build failure must
            // not block app startup.
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
            // arriving for >60s, the content process likely crashed (white screen).
            // Reload the webview to recover.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                // Give the webview time to load before starting watchdog
                std::thread::sleep(std::time::Duration::from_secs(30));
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(15));
                    let elapsed = handle.state::<LastHeartbeat>().0.lock().unwrap().elapsed();
                    if heartbeat_expired(elapsed) {
                        if let Some(ww) = handle.get_webview_window("main") {
                            eprintln!(
                                "[Tauri] WKWebView heartbeat timeout ({:.0}s) — reloading",
                                elapsed.as_secs_f64()
                            );
                            if let Ok(url) = ww.url() {
                                let _ = ww.navigate(url);
                            }
                            // Reset heartbeat so we don't reload in a loop
                            *handle.state::<LastHeartbeat>().0.lock().unwrap() = Instant::now();
                        }
                    }
                }
            });

            // Register the UserNotifications delegate + request notification
            // authorization. No-op in dev (unbundled binary). See notifications.rs.
            notifications::setup(app.handle());

            // Packaged build: boot the bundled Postgres + engine and point the
            // window at it. No-op in development (tauri-dev.sh supplies both).
            desktop::launch(app.handle());
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

/// Build and install the macOS app menu. Mirrors the default menu (so standard
/// shortcuts keep working) but maps Cmd+Q to "Close Window" (hides the client,
/// leaving the always-on service running) and exposes the deliberate full
/// teardown as a separate, unshortcutted "Quit & Stop Background Service" item
/// (routed through `on_menu_event` → `quit_lucidos`; also in the menu-bar tray).
fn install_app_menu(app: &tauri::App) -> tauri::Result<()> {
    let uninstall = MenuItem::with_id(
        app,
        "uninstall_lucidos",
        "Uninstall Lucidos…",
        true,
        None::<&str>,
    )?;
    // Cmd+Q closes the window (hides the client) rather than quitting — the
    // always-on service must survive it. The deliberate full teardown is the
    // separate, unshortcutted "Quit & Stop Background Service" (also in the tray).
    let close_window =
        MenuItem::with_id(app, "close_main_window", "Close Window", true, Some("Cmd+Q"))?;
    let quit = MenuItem::with_id(
        app,
        "quit_lucidos",
        "Quit & Stop Background Service",
        true,
        None::<&str>,
    )?;

    let app_menu = Submenu::with_items(
        app,
        "Lucidos",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("Lucidos"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &uninstall,
            &PredefinedMenuItem::separator(app)?,
            &close_window,
            &quit,
        ],
    )?;

    let new_window = MenuItem::with_id(app, "new_window", "New Window", true, Some("Cmd+N"))?;
    let file_menu = Submenu::with_items(app, "File", true, &[&new_window])?;

    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let menu = Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu, &window_menu])?;
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
    fn heartbeat_expired_fires_only_past_the_timeout() {
        // Below and at the threshold: not expired (15s heartbeat cadence).
        assert!(!heartbeat_expired(Duration::from_secs(59)));
        assert!(!heartbeat_expired(HEARTBEAT_TIMEOUT));
        // Strictly past the 60s timeout: reload.
        assert!(heartbeat_expired(Duration::from_secs(61)));
        assert!(heartbeat_expired(HEARTBEAT_TIMEOUT + Duration::from_millis(1)));
    }
}
