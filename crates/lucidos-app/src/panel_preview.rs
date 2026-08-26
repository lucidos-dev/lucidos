//! The desktop client's URL preview: a native child webview showing an
//! arbitrary remote page. See `docs/glossary.md` § "Panel preview".
//!
//! It is a child webview rather than an iframe for two reasons. WKWebView
//! cannot render an arbitrary remote page inside our own document, and most
//! sites refuse to be framed. That choice is what makes the family in ADR 0140
//! delicate: a window hosting a child webview stops answering tauri's
//! `WebviewWindow`-flavoured lookups, silently.
//!
//! **A preview belongs to one window, and is hosted on that same window.** The
//! child is added to `webview.window()`, the caller's own, so it is drawn over
//! the page that asked for it. Every command keys on its caller, so no page can
//! move, navigate, hide or close a preview it does not own.
//!
//! The page drives the whole life cycle over IPC. It asks for a preview, moves
//! it as its container resizes, hides it while an overlay is up, and closes it
//! on unmount. Two things the page cannot do are done here: reaping a preview
//! whose owner navigated or died, and routing the previewed page's own title
//! and URL reports back to the window that asked.

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::webview::PageLoadEvent;
use tauri::{Emitter, Manager, WebviewUrl};

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

/// Label prefix for a preview's child webview, the sibling of
/// `app_window::APP_WINDOW_PREFIX`.
///
/// The ACL boundary for arbitrary third-party content is scoped on this exact
/// string: `capabilities/panel-preview.json` globs `url-preview-*`. Renaming
/// the label without the capability denies the three report commands, so the
/// title and URL stop updating and a content read times out. Named here, and
/// the ACL tests build their fixtures from it, so a rename cannot pass green.
pub(crate) const PREVIEW_LABEL_PREFIX: &str = "url-preview-";

/// One `webview_get_content` waiting on the page it asked to read.
struct ContentRequest {
    /// Tells two extractions in flight from the SAME window apart.
    id: u64,
    /// The window that asked. A report names its window, not a request.
    owner: String,
    tx: std::sync::mpsc::Sender<(String, String)>,
}

/// Ids for [`ContentRequest`], unique for the life of the process.
static NEXT_CONTENT_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Every content extraction still waiting for an answer.
///
/// Keyed by REQUEST rather than by window, which a single shared sender and a
/// window-keyed map both got wrong. Two extractions can be in flight from one
/// window: `sendMessage` awaits `getWebviewContent()` outside the per-thread
/// send chain, so two sends overlap freely. The second registration dropped the
/// first's sender, and the first then cancelled the second's on its way out. So
/// BOTH callers lost the page they asked for.
#[derive(Default)]
pub(crate) struct PanelContentChannel(Mutex<Vec<ContentRequest>>);

impl PanelContentChannel {
    /// Register a waiting extraction and return the id that identifies it.
    fn open(&self, owner: &str, tx: std::sync::mpsc::Sender<(String, String)>) -> u64 {
        let id = NEXT_CONTENT_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.0.lock().unwrap().push(ContentRequest {
            id,
            owner: owner.to_string(),
            tx,
        });
        id
    }

    /// Drop one request, whatever became of it.
    ///
    /// By ID, which is the whole point. Removing by owner instead is what let a
    /// finished call cancel a concurrent one from the same window.
    fn close(&self, id: u64) {
        self.0.lock().unwrap().retain(|request| request.id != id);
    }

    /// Take the OLDEST extraction `owner` is waiting on.
    ///
    /// A report carries no request id, because the previewed page knows
    /// nothing about who asked. It answers whoever has waited longest, so two
    /// overlapping reads resolve in the order they were made.
    fn take_oldest_for(&self, owner: &str) -> Option<std::sync::mpsc::Sender<(String, String)>> {
        let mut pending = self.0.lock().unwrap();
        let index = pending.iter().position(|request| request.owner == owner)?;
        Some(pending.remove(index).tx)
    }
}

/// Every URL preview that is up, keyed by the app window whose page asked for
/// it. The value is that preview's own `url-preview-*` child webview label.
///
/// One key answers both questions the family used to get wrong. The owner
/// decides when a preview dies. It is also the HOST the child is added to, so a
/// preview is drawn over the page that asked for it.
#[derive(Default)]
pub(crate) struct PanelPreviewSlots(Mutex<HashMap<String, String>>);

impl PanelPreviewSlots {
    /// Record `child` as `owner`'s preview, handing back whatever it displaced
    /// so the caller can close it.
    ///
    /// Scoped to one owner, so a second window opening a preview cannot take
    /// down the one a first window is still showing.
    fn replace(&self, owner: &str, child: String) -> Option<String> {
        self.0.lock().unwrap().insert(owner.to_string(), child)
    }

    /// Take `owner`'s preview out of the map, if it has one.
    ///
    /// One matching take, never a check followed by a take. The preview closed
    /// is then the one the match was about, and not whatever replaced it in
    /// between.
    fn take(&self, owner: &str) -> Option<String> {
        self.0.lock().unwrap().remove(owner)
    }

    /// The child label `owner` has up, if it has one.
    fn child_of(&self, owner: &str) -> Option<String> {
        self.0.lock().unwrap().get(owner).cloned()
    }
}

/// The difference between the host window's logical height and the CSS viewport
/// height the calling page reported.
///
/// Reads the HOST window, which per-window hosting makes the caller's own. It
/// read `main` while every child was parked there, so a second window's preview
/// was offset by whatever gap `main` happened to have.
fn title_bar_gap(window: &tauri::Window, viewport_height: f64) -> f64 {
    let scale = window.scale_factor().unwrap_or(1.0);
    let window_h = window
        .inner_size()
        .map(|s| s.height as f64 / scale)
        .unwrap_or(0.0);
    (window_h - viewport_height).max(0.0)
}

/// The child webview `caller`'s own window has up, if it has one.
///
/// Every command that moves, navigates, hides, shows or reads a preview
/// resolves it through here, so each acts on the caller's own and no other.
fn callers_preview(app: &tauri::AppHandle, caller: &tauri::Webview) -> Option<tauri::Webview> {
    let label = app
        .state::<PanelPreviewSlots>()
        .child_of(caller.window().label())?;
    app.get_webview(&label)
}

/// Tear the child webview down, giving the previewed page its own cleanup hook
/// first. Takes a label rather than the state, so a caller that has already
/// taken the slot cannot forget half of the teardown.
fn close_preview_webview(app: &tauri::AppHandle, label: &str) {
    if let Some(wv) = app.get_webview(label) {
        let _ = wv.eval("if(window.__lucidos_title_cleanup) window.__lucidos_title_cleanup()");
        let _ = wv.close();
    }
}

/// Drop the panel preview `window_label`'s page owns, if it owns one.
///
/// The page that asked for a preview also closes it, on the unmount of the
/// component that positioned it. Two things run no unmount, and this covers
/// both: a NAVIGATION, and the window being destroyed. The child webview is
/// left behind, invisible while hidden, and the next overlay close draws it
/// over a page that has nowhere to put it.
pub(crate) fn close_owned_by(app: &tauri::AppHandle, window_label: &str) {
    if let Some(child) = app.state::<PanelPreviewSlots>().take(window_label) {
        close_preview_webview(app, &child);
    }
}

/// The box a preview fills, as the calling page measured it. One type for both
/// commands that place a preview, so the two cannot drift on what they take.
#[derive(serde::Deserialize)]
pub(crate) struct PreviewRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Where a preview goes, in the host window's own logical coordinates.
fn placement(rect: &PreviewRect, gap: f64) -> (tauri::Position, tauri::Size) {
    (
        tauri::Position::Logical(tauri::LogicalPosition::new(rect.x, rect.y + gap)),
        tauri::Size::Logical(tauri::LogicalSize::new(rect.width, rect.height + gap)),
    )
}

/// Open a URL preview for the calling page, replacing that page's own if it
/// already had one.
///
/// The caller decides everything: it owns the preview, and it HOSTS the child.
/// A preview is therefore drawn over the page that asked for it, which is what
/// makes owner-keying safe for every other command in this module.
#[tauri::command]
pub(crate) fn create_panel_webview(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    url: String,
    rect: PreviewRect,
    viewport_height: Option<f64>,
) -> Result<String, String> {
    let host = webview.window();
    let owner = host.label().to_string();

    let parsed_url: tauri::Url = url.parse().map_err(|e| format!("{e}"))?;
    let label = format!(
        "{PREVIEW_LABEL_PREFIX}{}",
        crate::app_window::next_webview_label_counter()
    );

    let gap = viewport_height
        .map(|vh| title_bar_gap(&host, vh))
        .unwrap_or(0.0);

    let emit_to = owner.clone();
    let page_load_app = app.clone();
    let builder = tauri::webview::WebviewBuilder::new(&label, WebviewUrl::External(parsed_url))
        .user_agent(safari_user_agent())
        .on_navigation(|_nav_url| true)
        .on_new_window(move |url, _features| {
            // The one site with nowhere to report to: a previewed page asked for
            // a window from inside the delegate, so there is no promise to reject
            // and no toast to raise. Log rather than discard.
            if let Err(e) = crate::open_in_default_browser(url.as_str()) {
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
                    // To the OWNER, captured here rather than named `main`. The
                    // page listens with its own window label, so a hardcoded
                    // target updated the wrong window's URL bar.
                    let _ = page_load_app.emit_to(&emit_to, "panel-url-changed", url);
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

    let (position, size) = placement(&rect, gap);
    host.add_child(builder, position, size)
        .map_err(|e| format!("{e}"))?;

    // Only after the child exists, so a failed build leaves the previous
    // preview recorded and closable rather than orphaned.
    if let Some(displaced) = app
        .state::<PanelPreviewSlots>()
        .replace(&owner, label.clone())
    {
        close_preview_webview(&app, &displaced);
    }

    Ok(label)
}

#[tauri::command]
pub(crate) fn navigate_panel_webview(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    url: String,
) -> Result<(), String> {
    let wv = callers_preview(&app, &webview).ok_or("panel webview not found")?;

    let parsed_url: tauri::Url = url.parse().map_err(|e| format!("{e}"))?;
    wv.navigate(parsed_url).map_err(|e| e.to_string())?;

    Ok(())
}

/// Close the calling page's own URL preview.
///
/// Owner-gated like the rest of the family, which per-window hosting is what
/// made safe. While every child was parked on `main`, a page could have a
/// preview drawn over it that another window owned. This was then the only
/// affordance left that could dismiss it. A preview now covers the page that
/// asked for it, so there is nothing to rescue and no reason to stay open.
#[tauri::command]
pub(crate) fn close_panel_webview(
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<(), String> {
    close_owned_by(&app, webview.window().label());
    Ok(())
}

#[tauri::command]
pub(crate) fn set_panel_webview_bounds(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    rect: PreviewRect,
    viewport_height: Option<f64>,
) -> Result<(), String> {
    let gap = viewport_height
        .map(|vh| title_bar_gap(&webview.window(), vh))
        .unwrap_or(0.0);
    if let Some(wv) = callers_preview(&app, &webview) {
        let (position, size) = placement(&rect, gap);
        wv.set_position(position).map_err(|e| format!("{e}"))?;
        wv.set_size(size).map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn hide_panel_webview(
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<(), String> {
    if let Some(wv) = callers_preview(&app, &webview) {
        wv.hide().map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn show_panel_webview(
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<(), String> {
    if let Some(wv) = callers_preview(&app, &webview) {
        wv.show().map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn webview_go_back(
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<(), String> {
    let wv = callers_preview(&app, &webview).ok_or("panel webview not found")?;
    wv.eval("window.history.back()").map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn webview_go_forward(
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<(), String> {
    let wv = callers_preview(&app, &webview).ok_or("panel webview not found")?;
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

/// Which app window a `__panel_*_report` belongs to.
///
/// These three commands are invoked BY the previewed page, so the calling
/// webview is the `url-preview-*` child. Its window is the host, and per-window
/// hosting makes the host the owner. That is the window whose page is waiting
/// to hear about this preview.
fn reporting_owner(caller: &tauri::Webview) -> String {
    caller.window().label().to_string()
}

#[tauri::command]
pub(crate) fn __panel_title_report(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    title: String,
) -> Result<(), String> {
    let _ = app.emit_to(reporting_owner(&webview), "panel-title-changed", title);
    Ok(())
}

#[tauri::command]
pub(crate) fn __panel_url_report(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    url: String,
) -> Result<(), String> {
    let _ = app.emit_to(reporting_owner(&webview), "panel-url-changed", url);
    Ok(())
}

/// Extract the text content and title from the caller's own preview. Evals JS
/// that calls `__panel_content_report`, which resolves a channel keyed by this
/// window. Runs on a blocking thread, so the main thread is free while it waits.
#[tauri::command]
pub(crate) async fn webview_get_content(
    app: tauri::AppHandle,
    webview: tauri::Webview,
) -> Result<serde_json::Value, String> {
    let owner = webview.window().label().to_string();
    let wv = callers_preview(&app, &webview).ok_or("panel webview not found")?;

    let (tx, rx) = std::sync::mpsc::channel();
    let id = app.state::<PanelContentChannel>().open(&owner, tx);

    let outcome = wv
        .eval(
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
        .map_err(|e| e.to_string())
        .and_then(|()| {
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|_| "content extraction timed out".to_string())
        });

    // Every path, so neither a failed eval nor a timeout parks a dead sender
    // that a later report would resolve against and discard the content.
    app.state::<PanelContentChannel>().close(id);
    let (title, content) = outcome?;

    Ok(serde_json::json!({ "title": title, "content": content }))
}

#[tauri::command]
pub(crate) fn __panel_content_report(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    title: String,
    content: String,
) -> Result<(), String> {
    let sender = app
        .state::<PanelContentChannel>()
        .take_oldest_for(&reporting_owner(&webview));
    if let Some(tx) = sender {
        let _ = tx.send((title, content));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A preview belongs to the page that asked for it, and only that page's
    /// navigation or destroy ends it.
    ///
    /// The single process-wide slot this replaced could not express that. A
    /// second window opening a preview silently took down the first window's,
    /// and either window's unmount closed whichever was up.
    #[test]
    fn one_windows_preview_is_untouched_by_another_windows() {
        let slots = PanelPreviewSlots::default();
        assert!(slots.replace("main", "url-preview-1".into()).is_none());
        assert!(slots.replace("window-2", "url-preview-2".into()).is_none());

        // Both are up at once, each answering for its own owner.
        assert_eq!(slots.child_of("main").as_deref(), Some("url-preview-1"));
        assert_eq!(slots.child_of("window-2").as_deref(), Some("url-preview-2"));

        // Taking one leaves the other exactly where it was.
        assert_eq!(slots.take("window-2").as_deref(), Some("url-preview-2"));
        assert_eq!(slots.child_of("main").as_deref(), Some("url-preview-1"));
        assert!(slots.child_of("window-2").is_none());
    }

    /// Opening a second preview in the SAME window replaces that window's own,
    /// and hands the displaced child back so its webview is closed.
    ///
    /// Dropping it on the floor would leave an orphan child attached and
    /// invisible, which the next overlay close draws over the page.
    #[test]
    fn a_second_preview_in_one_window_displaces_its_first_and_reports_it() {
        let slots = PanelPreviewSlots::default();
        slots.replace("main", "url-preview-1".into());
        assert_eq!(
            slots.replace("main", "url-preview-9".into()).as_deref(),
            Some("url-preview-1"),
            "the displaced child must come back so the caller can close it"
        );
        assert_eq!(slots.child_of("main").as_deref(), Some("url-preview-9"));
    }

    /// A window with no preview answers nothing rather than reaching for
    /// somebody else's, which is what every command's gate rests on.
    #[test]
    fn a_window_with_no_preview_answers_nothing() {
        let slots = PanelPreviewSlots::default();
        assert!(slots.child_of("window-3").is_none());
        assert!(slots.take("window-3").is_none());
        slots.replace("main", "url-preview-1".into());
        assert!(slots.child_of("window-3").is_none());
        assert!(slots.take("window-3").is_none());
        assert_eq!(slots.child_of("main").as_deref(), Some("url-preview-1"));
    }

    /// Two extractions in flight from ONE window each get their own answer.
    ///
    /// This is the case a window-keyed channel could not express, and it is
    /// reachable: `sendMessage` awaits `getWebviewContent()` outside the
    /// per-thread send chain, so two sends overlap. Keyed by window, the second
    /// registration dropped the first's sender and the first then removed the
    /// second's, so neither send carried the page.
    #[test]
    fn two_reads_from_one_window_do_not_cancel_each_other() {
        let channel = PanelContentChannel::default();
        let (tx_a, rx_a) = std::sync::mpsc::channel();
        let (tx_b, rx_b) = std::sync::mpsc::channel();
        let a = channel.open("main", tx_a);
        let b = channel.open("main", tx_b);
        assert_ne!(a, b, "two requests must not share an id");

        // Oldest first, so overlapping reads resolve in the order they were made.
        let first = channel.take_oldest_for("main").expect("a waiting request");
        first.send(("A".into(), "page A".into())).unwrap();
        assert_eq!(rx_a.try_recv().unwrap().0, "A");

        // A finishing does NOT take B's channel down with it.
        channel.close(a);
        let second = channel.take_oldest_for("main").expect("B is still waiting");
        second.send(("B".into(), "page B".into())).unwrap();
        assert_eq!(rx_b.try_recv().unwrap().0, "B");
        channel.close(b);
    }

    /// A report answers the window that asked, and no other.
    #[test]
    fn a_report_resolves_only_its_own_windows_read() {
        let channel = PanelContentChannel::default();
        let (tx_main, rx_main) = std::sync::mpsc::channel();
        let (tx_two, rx_two) = std::sync::mpsc::channel();
        channel.open("main", tx_main);
        channel.open("window-2", tx_two);

        channel
            .take_oldest_for("window-2")
            .expect("window-2 is waiting")
            .send(("two".into(), String::new()))
            .unwrap();
        assert_eq!(rx_two.try_recv().unwrap().0, "two");
        assert!(rx_main.try_recv().is_err(), "main's read is untouched");
        assert!(channel.take_oldest_for("main").is_some());
    }

    /// A window nobody is reading for answers nothing, rather than reaching for
    /// somebody else's request.
    #[test]
    fn a_report_with_no_waiting_read_is_a_no_op() {
        let channel = PanelContentChannel::default();
        assert!(channel.take_oldest_for("main").is_none());
        let (tx, _rx) = std::sync::mpsc::channel();
        let id = channel.open("window-2", tx);
        assert!(channel.take_oldest_for("main").is_none());
        // Closing an id twice, or one already taken, is harmless.
        channel.close(id);
        channel.close(id);
        assert!(channel.take_oldest_for("window-2").is_none());
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
}
