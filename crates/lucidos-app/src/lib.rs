use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::PageLoadEvent;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

mod activation;
mod app_window;
mod config_scalar;
mod crash_watchdog;
mod desktop;
mod device_id_store;
mod mobile;
mod notifications;
mod pairing;
mod panel_preview;
/// Login-shell environment hydration for a GUI launch. macOS-only: it exists
/// because launchd hands a packaged process an environment the user's profile
/// never touched, which is a macOS packaging fact.
#[cfg(target_os = "macos")]
mod shell_env;
mod traffic_lights;
mod updater;
mod window_persist;
mod window_restore;
mod window_session;
mod window_target;

/// Headless launchd entry point — `Lucidos --service` (see `desktop::run_service`).
/// Boots the bundled Postgres + engine and supervises them with no window. The
/// caller (`main`) routes the process here before any Tauri init.
pub fn run_service() -> i32 {
    desktop::run_service()
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
pub(crate) fn open_in_default_browser(url: &str) -> Result<(), String> {
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

/// Set true only by the explicit full-teardown path ("Quit & Stop Background
/// Service" — `quit_lucidos`) so the `ExitRequested` handler lets that
/// `app.exit(0)` through. Otherwise that handler prevents the auto-exit a
/// last-window close would trigger, keeping the client resident in the menu bar
/// while the always-on launchd service runs untouched. Packaged only.
static QUITTING: AtomicBool = AtomicBool::new(false);

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
pub(crate) fn titlebar_inset_script() -> String {
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

/// Paint the macOS window background of every top-level app window the given
/// color. Sets the WINDOW-layer background only, never the webview's, so the
/// page background is never tinted and there is no load flash. Panel preview
/// webviews are skipped, and a failure on one window is logged, not fatal.
pub(crate) fn paint_title_bars(app: &tauri::AppHandle, color: tauri::utils::config::Color) {
    for (label, window) in app.windows() {
        if app_window::is_app_window(&label) {
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
pub(crate) fn pre_paint_title_bar_color(
    app: &tauri::AppHandle,
) -> Option<tauri::utils::config::Color> {
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
    // an in-session move or resize is lost across the restart. The session
    // record goes with it, or the restart comes back with one window.
    window_persist::persist_windows(&app);
    window_persist::begin_teardown();

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
        window_persist::persist_windows(&handle);
        window_persist::begin_teardown();
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
            // Everything from here goes over to the MAIN thread, in one
            // closure. This callback runs on a worker: `tauri-plugin-dialog`
            // spawns one to await the sheet. And `save_window_state` deadlocks
            // off the main thread (see `persist_window_state_on_main`).
            //
            // One closure rather than three hops, because the order is the
            // point. The record must be on disk, and the teardown flag set,
            // before the exit destroys the windows.
            let handle = quit_app.clone();
            if let Err(e) = quit_app.run_on_main_thread(move || {
                window_persist::persist_windows(&handle);
                window_persist::begin_teardown();
                desktop::stop_service();
                handle.exit(0);
            }) {
                // The event loop is unreachable. Quit anyway, without the
                // record: refusing to quit is worse than forgetting a window.
                eprintln!("[Tauri] Could not marshal the quit onto the main thread: {e}");
                desktop::stop_service();
                quit_app.exit(0);
            }
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
                    app_window::hide_all_windows(&fate_app);
                    run_uninstall(fate_app.clone(), delete_data);
                });
        });
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
    app_window::show_main_window(app);
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
///
/// By window, not webview window, per ADR 0140. Showing is a window operation.
/// No preview can exist this early, so the flavour costs nothing here and is
/// simply the one the rule asks for.
fn show_startup_window(app: &tauri::AppHandle) -> bool {
    if !STARTUP_SHOW.claim(activation::is_menu_bar_only()) {
        return false;
    }
    match app.get_window(app_window::MAIN_WINDOW_LABEL) {
        Some(win) => match win.show() {
            // Only a window that actually reached the screen latches the gate,
            // the same rule `front_window` follows.
            Ok(()) => window_persist::note_presented(),
            Err(e) => eprintln!("[Tauri] Failed to show the main window: {e}"),
        },
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
                app_window::reopen_client(app);
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
                .with_state_flags(window_persist::window_state_flags())
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(panel_preview::PanelPreviewSlots::default())
        .manage(panel_preview::PanelContentChannel::default())
        .manage(crash_watchdog::LastHeartbeat::default())
        .manage(window_persist::GeometrySaver::default())
        .manage(device_id_store::DeviceIdStore::default())
        .manage(updater::AppUpdateRun::default())
        .manage(mobile::MobileAccessRuns::default())
        .manage(desktop::StartupStatus::default())
        .manage(DockBadgeNudge(Mutex::new(dock_badge_nudge_tx)))
        .invoke_handler(tauri::generate_handler![
            panel_preview::create_panel_webview,
            panel_preview::navigate_panel_webview,
            panel_preview::close_panel_webview,
            panel_preview::set_panel_webview_bounds,
            panel_preview::hide_panel_webview,
            panel_preview::show_panel_webview,
            panel_preview::webview_go_back,
            panel_preview::webview_go_forward,
            panel_preview::__panel_title_report,
            panel_preview::__panel_url_report,
            panel_preview::webview_get_content,
            panel_preview::__panel_content_report,
            crash_watchdog::heartbeat,
            startup_status,
            restart_app,
            restart_service,
            quit_lucidos,
            uninstall_lucidos,
            open_url_external,
            save_to_downloads,
            show_native_notification,
            dismiss_native_notification,
            app_window::focus_calling_window,
            nudge_dock_badge,
            app_window::get_native_window_active,
            take_pending_native_taps,
            updater::check_app_update,
            updater::install_app_update_and_restart,
            updater::cancel_app_update,
            set_titlebar_color,
            set_traffic_light_offset,
            window_ready_to_show,
            app_window::start_window_drag,
            app_window::toggle_window_maximize,
            app_window::set_window_title,
            app_window::show_workspace_window,
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
                    window_persist::persist_window_state_only(app);
                    // The SESSION only when this close is the user's own. A
                    // teardown queues one of these per window, and by the
                    // second the first is already half torn down: its getters
                    // fail and the capture would drop it.
                    if !window_persist::tearing_down() {
                        window_persist::persist_window_session(app);
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
                        app_window::emit_window_active(app, window.label(), false);
                        app_window::enter_menu_bar_only_if_no_windows(app, Some(window.label()));
                    }
                }
                // A secondary window closed. Drop its traffic-light resize
                // observer, then re-evaluate the tray. The tray half is
                // packaged-only; the observer half is not, because a leaked
                // registration is keyed on the dead window's address in both
                // builds. `main` never reaches here: its close is prevented.
                tauri::WindowEvent::Destroyed if app_window::is_app_window(window.label()) => {
                    traffic_lights::unwatch(window.label());
                    // The SLOT is what this clears. The child webview itself
                    // dies with its host, which is now this window, and tauri
                    // drops it from the manager in `on_window_close`. Left
                    // behind, the map would keep one entry per destroyed
                    // window that had a preview up.
                    panel_preview::close_owned_by(app, window.label());
                    // The window is out of the map by now, so re-recording is
                    // what drops it from the session. `CloseRequested` cannot
                    // do it: the window is still there when that one fires.
                    window_persist::forget_closed_window(app);
                    if !tauri::is_dev() {
                        app_window::enter_menu_bar_only_if_no_windows(app, Some(window.label()));
                    }
                }
                // Bridge native focus so `isPageActive()` reads the AppKit state
                // rather than the flaky WKWebView `hasFocus()`. The trayed case
                // is covered by the explicit emit in `CloseRequested` above.
                tauri::WindowEvent::Focused(focused)
                    if app_window::is_app_window(window.label()) =>
                {
                    app_window::emit_window_active(app, window.label(), *focused);
                }
                // Arm the debounced background flush. The plugin keeps its own
                // in-memory cache from these same events, and the disk write is
                // what makes the geometry survive a relaunch.
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
                    if app_window::is_app_window(window.label()) =>
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
                    window_persist::note_geometry_changed(app);
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
                app_window::close_all_to_tray(app);
            } else if event.id() == "uninstall_lucidos" {
                uninstall_lucidos(app.clone());
            } else if event.id() == "new_window" {
                if let Err(e) = app_window::open_new_window(app) {
                    eprintln!("[Tauri] Failed to open new window: {e}");
                }
            }
        })
        .on_page_load(|webview, payload| {
            if app_window::is_app_window(webview.label())
                && matches!(payload.event(), PageLoadEvent::Started)
            {
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
                // The outgoing page took its preview's positioning element with
                // it, and ran no unmount to say so.
                panel_preview::close_owned_by(webview.app_handle(), webview.window().label());
            }
        })
        .setup(move |app| {
            // What this launch owes the user: a window per workspace that had
            // one, at the size that workspace was left. Resolved BEFORE the
            // clamp and the show, because `main` takes the first entry and must
            // be sized before it appears rather than resized after.
            let plan = window_persist::resolve_window_session_plan();
            if let Some((_, Some(frame))) = plan.first() {
                window_persist::size_main_window_for_its_workspace(app.handle(), *frame);
            }

            // The plugin has already written the saved rect onto `main`, and
            // nothing has shown the window yet. This is therefore the one moment
            // a corrupt or now-impossible rect can be corrected off screen. A
            // healthy rect makes it a no-op. It runs AFTER the line above, so it
            // judges the rect the window will actually wear. See
            // `window_restore`.
            window_restore::clamp_restored_geometry(app.handle(), app_window::MAIN_WINDOW_LABEL);

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
                activation::set_menu_bar_only(app.handle(), true);
            }

            crash_watchdog::spawn(app.handle().clone());

            window_persist::spawn_geometry_flush(app.handle().clone());

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
                    } else {
                        // An exit that is going ahead. Record the arrangement
                        // before the teardown destroys the windows, and stop
                        // re-recording once it does. See `window_persist::tearing_down`.
                        window_persist::persist_windows(app_handle);
                        window_persist::begin_teardown();
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
                        && activation::reopen_shows_a_window(app_window::visible_app_windows(
                            app_handle, None,
                        ))
                    {
                        app_window::reopen_client(app_handle);
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

    /// Every `.rs` under `src/`, read at test time rather than baked in with
    /// `include_str!`, which cannot take a computed path. A hardcoded list is
    /// one new module away from a silent gap, and this crate just grew five.
    ///
    /// RECURSIVE, which the two gates below depend on. Every module is a
    /// top-level file today. So a flat read passes right now, and goes blind
    /// the day one of them becomes a directory.
    fn crate_source_files() -> Vec<std::path::PathBuf> {
        fn collect(dir: &std::path::Path, into: &mut Vec<std::path::PathBuf>) {
            let listing = std::fs::read_dir(dir)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
            for entry in listing {
                let path = entry.expect("a readable dir entry").path();
                if path.is_dir() {
                    collect(&path, into);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    into.push(path);
                }
            }
        }
        let mut files = Vec::new();
        collect(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        files.sort();
        assert!(!files.is_empty(), "scanned no source files");
        files
    }

    /// No manager lookup asks for a `WebviewWindow` either: the other half of
    /// ADR 0140, and the half that stayed unenforced until the sweep landed.
    ///
    /// Both flavours answer as though a preview-hosting window did not exist,
    /// and both do it in silence: `None`, or a map with the window missing.
    /// Widening this gate is what finishing the sweep bought. The ADR shipped
    /// with it argument-only, because twelve sites would have red-lighted the
    /// crate, and a gate that has to be switched off teaches nothing.
    ///
    /// **Per-window hosting made this bind harder, not softer.** Any app window
    /// can host a child now, so a blind lookup no longer merely misses `main`.
    /// It misses whichever window the user last opened a link in.
    ///
    /// The needles are built from parts, so this test's own source does not
    /// match itself and neither does the prose above.
    #[test]
    fn no_manager_lookup_asks_for_a_webview_window() {
        let needles = [
            concat!("get_", "webview_window("),
            concat!(".webview_", "windows()"),
        ];
        let mut hits = Vec::new();
        for path in crate_source_files() {
            let source = std::fs::read_to_string(&path).expect("a readable source file");
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            for (number, line) in source.lines().enumerate() {
                for needle in needles {
                    if line.contains(needle) {
                        hits.push(format!("{file}:{}: {}", number + 1, line.trim()));
                    }
                }
            }
        }
        assert!(
            hits.is_empty(),
            "these lookups are blind to a window hosting a URL preview (ADR 0140). \
             Take a tauri::Window for a window operation, or a tauri::Webview for a \
             page one:\n  {}",
            hits.join("\n  ")
        );
    }

    /// No command may declare a `tauri::WebviewWindow` parameter: ADR 0140.
    ///
    /// Tauri refuses that argument outright while a URL preview is open, so
    /// the command is dead and the page gets a string naming no command. A
    /// scan, because the failure is invisible at every call site: the argument
    /// is not passed, and the declaration reads correctly.
    #[test]
    fn no_command_takes_a_webview_window() {
        let files = crate_source_files();
        // Short of the closing bracket, so the `(async)` spelling matches too.
        let needle = "#[tauri::command";
        let mut checked = 0;
        for path in &files {
            let source = std::fs::read_to_string(path).expect("a readable source file");
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            for (index, _) in source.match_indices(needle) {
                // Only a real attribute opens a line, indentation aside. This
                // needle and the prose naming it sit mid-line, so the scan
                // skips them rather than reading them as declarations.
                let head = &source[..index];
                let before = head.rfind('\n').map_or(head, |i| &head[i + 1..]);
                if !before.trim().is_empty() {
                    continue;
                }
                let after = &source[index..];
                let signature = after.split_once('{').map_or(after, |(head, _)| head);
                assert!(
                    !signature.contains("WebviewWindow"),
                    "{file}: `{}` takes a WebviewWindow, which a window hosting a URL \
                     preview cannot supply",
                    signature.lines().nth(1).unwrap_or_default().trim()
                );
                checked += 1;
            }
        }
        // The registered set, which is the count that has to be covered.
        let registered = crate::acl_tests::invoke_handler_commands().len();
        assert_eq!(
            checked, registered,
            "scanned {checked} declarations against {registered} registered commands"
        );
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
    ///
    /// Shared with `tests::no_command_takes_a_webview_window`, which counts its
    /// own scan against this to prove the scan saw every command.
    pub(crate) fn invoke_handler_commands() -> BTreeSet<String> {
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

    /// Every window a preview can be hosted on, as the ACL judges it.
    ///
    /// A child is added to its OWNER's window now, rather than parked on
    /// `main`. So a second window is a real host rather than a hypothetical
    /// one. The capability scopes by `webviews` and leaves `windows` empty, so
    /// the host must not change the answer. These two rows are what says so.
    const PREVIEW_HOSTS: [&str; 2] = ["main", "window-2"];

    /// A child webview's label, built from the constant the code mints it with.
    ///
    /// Not a literal, because `capabilities/panel-preview.json` globs on that
    /// same prefix. Hardcode it here and renaming the label leaves these tests
    /// green while the capability silently stops matching.
    fn preview_label(n: u32) -> String {
        format!("{}{n}", crate::panel_preview::PREVIEW_LABEL_PREFIX)
    }

    #[test]
    fn panel_previews_get_the_three_report_commands_and_nothing_else() {
        let authority = authority(Some(PORT));
        // A preview webview lives INSIDE an app window and shows an arbitrary
        // third-party page, so it is judged as (window: the host, webview:
        // url-preview-N) on that page's origin.
        let previewed = remote("https://example.com");
        for host in PREVIEW_HOSTS {
            for command in [
                "__panel_title_report",
                "__panel_url_report",
                "__panel_content_report",
            ] {
                assert!(
                    authority
                        .resolve_access(command, host, &preview_label(3), &previewed)
                        .is_some(),
                    "`{command}` is denied in a preview hosted on {host}, so the previewed \
                     page's title, URL and text would never reach the window showing it"
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
                        .resolve_access(command, host, &preview_label(3), &previewed)
                        .is_none(),
                    "`{command}` is reachable from previewed content hosted on {host}"
                );
            }
        }
    }

    #[test]
    fn the_app_surface_is_not_reachable_from_a_preview_on_the_gateway_origin() {
        // Same-origin previews are the sharp edge of scoping by `windows`: if the
        // gateway capability were window-scoped, a preview of a localhost page
        // would inherit its host window's full grant.
        let authority = authority(Some(PORT));
        let origin = Origin::Remote {
            url: gateway_origin(),
        };
        for host in PREVIEW_HOSTS {
            assert!(
                authority
                    .resolve_access("heartbeat", host, &preview_label(1), &origin)
                    .is_none(),
                "a preview webview inherited {host}'s app IPC grant"
            );
        }
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
