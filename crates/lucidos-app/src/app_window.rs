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
    window_target,
};

/// Put a window at `frame`, in the physical pixels the record stores.
///
/// Both restore paths go through here: `main` before the deferred show, and
/// each extra window while it is still hidden. Best-effort and logged, since a
/// window at the wrong size beats no window.
/// Takes a `tauri::Window`, per ADR 0140: sizing and placing are window
/// operations. Both callers already hold one, the restore path from a lookup
/// and the builder path off the window it just built.
pub(crate) fn place_window(window: &tauri::Window, frame: window_restore::Rect, what: &str) {
    if let Err(e) = window.set_size(tauri::PhysicalSize::new(
        frame.width as u32,
        frame.height as u32,
    )) {
        eprintln!("[Tauri] Failed to size {what}: {e}");
    }
    if let Err(e) =
        window.set_position(tauri::PhysicalPosition::new(frame.x as i32, frame.y as i32))
    {
        eprintln!("[Tauri] Failed to place {what}: {e}");
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
/// `frame` is the geometry the window's WORKSPACE was last left at, in physical
/// pixels. Such a window is built hidden, placed, and shown once it is right, so
/// it never appears at the default size and jumps. `None` takes the declared
/// default, centred: File > New Window, and a workspace nothing is remembered
/// about.
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
    let builder = WebviewWindowBuilder::new(app, &label, url)
        .title("Lucidos")
        .inner_size(1024.0, 768.0)
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
    // A restored window is built HIDDEN and shown at the end. It then appears
    // at its own frame rather than at the default and jumping.
    //
    // The builder cannot carry the frame itself: its `inner_size` and
    // `position` are LOGICAL pixels and the record is physical. Converting
    // needs the scale factor of the monitor the window lands on, which no one
    // knows before it exists.
    let builder = if frame.is_some() {
        builder.visible(false)
    } else {
        builder
    };
    let window = builder.build().map_err(|e| format!("{e}"))?;
    if let Some(frame) = frame {
        place_window(
            &window.as_ref().window(),
            frame,
            &format!("the restored window {label}"),
        );
        // Same sanity pass `main` gets: a frame saved against a display that is
        // no longer attached must not put a window somewhere unreachable.
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
    if frame.is_some() {
        if let Err(e) = window.show() {
            eprintln!("[Tauri] Failed to show the restored window {label}: {e}");
        }
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
    for window in app.windows().values() {
        let _ = window.hide();
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
    // lands. Place then clamp, the pair `setup` uses. The clamp rides the frame
    // here, unlike in `setup`: with no frame to place, `main` keeps the geometry
    // launch already sanitised.
    if let Some(planned) = &plan.navigate_main {
        desktop::navigate_main_window(app, &planned.url);
        if let Some(frame) = planned.frame {
            window_persist::size_main_window_for_its_workspace(app, frame);
            window_restore::clamp_restored_geometry(app, MAIN_WINDOW_LABEL);
        }
    }
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
            if let Err(e) = window.show() {
                eprintln!("[Tauri] Failed to show the parked window {label}: {e}");
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
    // By window, not webview window, per ADR 0140. Fronting is a pure window
    // operation, and the lookup must survive a URL preview open in the target.
    if let Some(window) = app.get_window(label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        activation::activate_app_frontmost();
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
