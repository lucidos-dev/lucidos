//! Keeps a window that has reached the gateway off the bundled boot splash.
//!
//! WKWebView takes a mouse Back button as a history step, and the first entry of
//! every launch window is the splash. The splash only waits for
//! `desktop::launch`, which navigates once and exits. A window sent back there
//! would wait for good, in front of a healthy gateway.
//!
//! The guard keys on what each webview has LOADED, never on its current URL.
//! During a native Back, `WKWebView.URL` can already report the pending target.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tauri::plugin::TauriPlugin;
use tauri::webview::PageLoadEvent;
use tauri::Runtime;

use crate::app_window::is_app_window;
use crate::window_target::{is_bundled_app_url, window_is_navigated};

/// The app windows, by label, that have loaded a gateway page. Only app windows
/// start on the splash. A preview's label is unique, so recording one would
/// grow the set with every preview opened.
#[derive(Default)]
struct ReachedGateway(Mutex<HashSet<String>>);

impl ReachedGateway {
    fn note_load(&self, label: &str, url: &str) {
        if is_app_window(label) && window_is_navigated(url) {
            self.labels().insert(label.to_string());
        }
    }

    fn allows(&self, label: &str, target: &str) -> bool {
        !(is_bundled_app_url(target) && self.labels().contains(label))
    }

    fn labels(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        // A set of labels stays whole even if a holder panicked mid-insert.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let reached = Arc::new(ReachedGateway::default());
    let loads = Arc::clone(&reached);
    tauri::plugin::Builder::new("boot-splash-guard")
        .on_page_load(move |webview, payload| {
            if matches!(payload.event(), PageLoadEvent::Started) {
                loads.note_load(webview.label(), payload.url().as_str());
            }
        })
        .on_navigation(move |webview, target| reached.allows(webview.label(), target.as_str()))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPLASH: &str = "tauri://localhost";
    const WORKSPACE: &str = "http://localhost:3210/myws/";

    #[test]
    fn a_window_that_reached_the_gateway_never_goes_back_to_the_splash() {
        let reached = ReachedGateway::default();
        reached.note_load("main", SPLASH);
        reached.note_load("main", WORKSPACE);
        assert!(!reached.allows("main", SPLASH));
        assert!(!reached.allows("main", "tauri://localhost/index.html"));
    }

    #[test]
    fn a_window_still_booting_may_load_and_reload_the_splash() {
        let reached = ReachedGateway::default();
        reached.note_load("main", SPLASH);
        assert!(reached.allows("main", SPLASH));
        assert!(reached.allows("main", WORKSPACE));
    }

    #[test]
    fn one_window_reaching_the_gateway_leaves_the_others_alone() {
        let reached = ReachedGateway::default();
        reached.note_load("window-1", WORKSPACE);
        assert!(reached.allows("main", SPLASH));
    }

    #[test]
    fn a_preview_webview_is_never_recorded() {
        let reached = ReachedGateway::default();
        reached.note_load("url-preview-7", WORKSPACE);
        assert!(reached.labels().is_empty());
    }

    #[test]
    fn a_window_on_the_gateway_still_loads_its_pages_and_frames() {
        let reached = ReachedGateway::default();
        reached.note_load("main", WORKSPACE);
        for target in [
            "http://localhost:3210/other/",
            "about:blank",
            "about:srcdoc",
            "blob:http://localhost:3210/x",
        ] {
            assert!(reached.allows("main", target), "{target}");
        }
    }
}
