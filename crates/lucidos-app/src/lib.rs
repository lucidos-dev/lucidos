use std::sync::atomic::AtomicU32;
use std::sync::Mutex;
use std::time::Instant;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::webview::PageLoadEvent;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

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

/// Build a Safari-like user-agent from the actual system Safari version.
/// WKWebView's default UA omits the `Version/X.Y Safari/605.1.15` suffix,
/// making Google Docs (and others) think it's an unsupported browser.
/// Cached via `OnceLock` so the `defaults` process only spawns once.
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
            .unwrap_or_else(|| "18.0".to_string());

        format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/605.1.15 (KHTML, like Gecko) \
             Version/{safari_version} Safari/605.1.15"
        )
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

/// Counter for generating unique webview/window labels.
static WEBVIEW_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Label prefix for additional top-level app windows opened via File → New
/// Window. The first window is `main` (declared in `tauri.conf.json`); each
/// extra window gets `window-<n>`. Panel preview webviews use the
/// `url-preview-<n>` prefix instead, so app-window-only setup (the app-version
/// injection in `on_page_load`) can tell the two apart.
const APP_WINDOW_PREFIX: &str = "window-";

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

/// Open an additional top-level app window (File → New Window / Cmd+N). Every
/// window is just another client of the same engine — the engine + Postgres run
/// as a shared launchd service (see `desktop`), so all windows share one
/// workspace stack. The WKWebView crash-recovery watchdog stays scoped to
/// `main`.
fn open_new_window(app: &tauri::AppHandle) -> Result<(), String> {
    let counter = WEBVIEW_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("{APP_WINDOW_PREFIX}{counter}");

    WebviewWindowBuilder::new(app, &label, new_window_url(app))
        .title("Lucidos")
        .inner_size(1024.0, 768.0)
        .build()
        .map_err(|e| format!("{e}"))?;
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

/// Fully quit Lucidos: stop the always-on gateway service (`launchctl bootout`),
/// then exit the GUI. This is the ONLY teardown path — window close leaves the
/// service running. The next app launch re-installs and re-bootstraps it.
#[tauri::command]
fn quit_lucidos(app: tauri::AppHandle) {
    desktop::stop_service();
    app.exit(0);
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
            open_url_external,
            show_native_notification,
            mobile::get_connect_info,
            mobile::tailscale_up,
            mobile::tailscale_serve,
        ])
        .on_menu_event(|app, event| {
            // "Quit Lucidos" stops the always-on service before exit; "New
            // Window" opens another client window; every other menu item keeps
            // its default behavior. Window close (red X / Cmd+W) is a window
            // event, not a menu item, so it leaves the service running.
            if event.id() == "quit_lucidos" {
                quit_lucidos(app.clone());
            } else if event.id() == "new_window" {
                if let Err(e) = open_new_window(app) {
                    eprintln!("[Tauri] Failed to open new window: {e}");
                }
            }
        })
        .on_page_load(|webview, payload| {
            if is_app_window(webview.label()) && matches!(payload.event(), PageLoadEvent::Started) {
                let version = env!("LUCIDOS_APP_VERSION");
                if let Err(e) =
                    webview.eval(format!("window.__LUCIDOS_APP_VERSION__ = '{}';", version))
                {
                    eprintln!("[Tauri] Failed to inject app version: {e}");
                }
            }
        })
        .setup(|app| {
            // Install the app menu. The standard edit/window items keep the
            // usual shortcuts (Cmd+C/V, minimize…); the app submenu's Quit item
            // is a CUSTOM "Quit Lucidos" (id `quit_lucidos`) so on_menu_event can
            // stop the always-on service on an explicit quit — while window
            // close leaves it running. Best-effort: a menu build failure must
            // not block app startup.
            if let Err(e) = install_app_menu(app) {
                eprintln!("[Tauri] Failed to install app menu: {e}");
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
                    if elapsed > std::time::Duration::from_secs(60) {
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
            // Packaged build: check GitHub Releases for an update in the
            // background and prompt to restart if one is available.
            updater::check_on_startup(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_handle, _event| {
            // The engine + Postgres run as a launchd service, independent of the
            // window — so GUI exit (window close, Cmd+W) intentionally tears
            // nothing down. The only stop path is the explicit "Quit Lucidos"
            // menu item (see `quit_lucidos` / on_menu_event).
        });
}

/// Build and install the macOS app menu. Mirrors the default menu (so standard
/// shortcuts keep working) but replaces the app submenu's predefined Quit with
/// a custom "Quit Lucidos" item routed through `on_menu_event` → `quit_lucidos`.
fn install_app_menu(app: &tauri::App) -> tauri::Result<()> {
    let quit = MenuItem::with_id(app, "quit_lucidos", "Quit Lucidos", true, Some("Cmd+Q"))?;

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
