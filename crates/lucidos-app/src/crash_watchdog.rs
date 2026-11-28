use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::Manager;

#[derive(Default)]
pub(crate) struct Heartbeats {
    beats: Mutex<HashMap<String, Beat>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Pulse {
    count: u64,
    page: Option<String>,
}

impl Pulse {
    fn is_unchanged_since(&self, earlier: &Pulse) -> bool {
        match (&self.page, &earlier.page) {
            (Some(now), Some(then)) => now == then,
            _ => self.count == earlier.count,
        }
    }
}

struct Beat {
    at: Instant,
    pulse: Pulse,
    visible: Option<bool>,
}

impl Beat {
    fn new() -> Self {
        Beat {
            at: Instant::now(),
            pulse: Pulse::default(),
            visible: None,
        }
    }
}

struct Reading {
    silent_for: Duration,
    pulse: Pulse,
    page_reported_hidden: bool,
}

impl Heartbeats {
    fn record(&self, label: &str, page: Option<String>, visible: Option<bool>) {
        let mut beats = self.beats.lock().unwrap();
        let beat = beats.entry(label.to_string()).or_insert_with(Beat::new);
        beat.at = Instant::now();
        beat.pulse.count += 1;
        beat.pulse.page = page;
        beat.visible = visible;
    }

    fn read(&self, label: &str) -> Reading {
        let mut beats = self.beats.lock().unwrap();
        let beat = beats.entry(label.to_string()).or_insert_with(Beat::new);
        Reading {
            silent_for: beat.at.elapsed(),
            pulse: beat.pulse.clone(),
            page_reported_hidden: beat.visible == Some(false),
        }
    }

    fn reset_clock(&self, label: &str) {
        if let Some(beat) = self.beats.lock().unwrap().get_mut(label) {
            beat.at = Instant::now();
        }
    }

    fn labels(&self) -> Vec<String> {
        self.beats.lock().unwrap().keys().cloned().collect()
    }

    fn forget(&self, label: &str) {
        self.beats.lock().unwrap().remove(label);
    }
}

const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(60);

const WATCHDOG_TICK: Duration = Duration::from_secs(15);

const WARMUP: Duration = Duration::from_secs(30);

const MAX_RELOAD_BACKOFF_DOUBLINGS: u32 = 5;

fn reload_threshold(futile_reloads: u32) -> Duration {
    HEARTBEAT_TIMEOUT * 2u32.pow(futile_reloads.min(MAX_RELOAD_BACKOFF_DOUBLINGS))
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Parked,
    Watching,
    Reload(ReloadDecision),
}

#[derive(Debug, PartialEq, Eq)]
struct ReloadDecision {
    futile: bool,
    next_threshold: Duration,
}

#[derive(Debug, Default)]
struct ReloadWatchdog {
    pulse_at_last_reload: Option<Pulse>,
    futile_reloads: u32,
}

impl ReloadWatchdog {
    fn on_tick(&mut self, on_screen: bool, silent_for: Duration, pulse: Pulse) -> Verdict {
        if !on_screen {
            *self = Self::default();
            return Verdict::Parked;
        }
        if silent_for <= reload_threshold(self.futile_reloads) {
            return Verdict::Watching;
        }
        let futile = self
            .pulse_at_last_reload
            .as_ref()
            .is_some_and(|earlier| pulse.is_unchanged_since(earlier));
        self.futile_reloads = if futile {
            self.futile_reloads.saturating_add(1)
        } else {
            0
        };
        self.pulse_at_last_reload = Some(pulse);
        Verdict::Reload(ReloadDecision {
            futile,
            next_threshold: reload_threshold(self.futile_reloads),
        })
    }
}

#[tauri::command]
pub(crate) fn heartbeat(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    page: Option<String>,
    visible: Option<bool>,
) {
    app.state::<Heartbeats>()
        .record(webview.label(), page, visible);
}

pub(crate) fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(WARMUP);
        let mut watchdogs: HashMap<String, ReloadWatchdog> = HashMap::new();
        loop {
            std::thread::sleep(WATCHDOG_TICK);
            tick(&app, &mut watchdogs);
        }
    });
}

fn tick(app: &tauri::AppHandle, watchdogs: &mut HashMap<String, ReloadWatchdog>) {
    let state = app.state::<Heartbeats>();
    let mut live = Vec::new();
    for (label, webview) in app.webviews() {
        if !crate::app_window::is_app_window(&label) {
            continue;
        }
        live.push(label.clone());
        let window = webview.window();
        let miniaturized = window.is_minimized().unwrap_or(false);
        let key_window = window.is_focused().unwrap_or(true);
        let Reading {
            silent_for,
            pulse,
            page_reported_hidden,
        } = state.read(&label);
        let on_screen = crate::window_screen::page_should_be_running(
            crate::window_screen::shown_by_client(&label),
            crate::window_screen::app_is_hidden(),
            miniaturized,
            page_reported_hidden,
            key_window,
        );
        let watchdog = watchdogs.entry(label.clone()).or_default();
        match watchdog.on_tick(on_screen, silent_for, pulse) {
            Verdict::Watching => {}
            Verdict::Parked => state.reset_clock(&label),
            Verdict::Reload(decision) => {
                report(&label, &decision, silent_for, watchdog.futile_reloads);
                if let Ok(url) = webview.url() {
                    let _ = webview.navigate(url);
                }
                state.reset_clock(&label);
            }
        }
    }
    watchdogs.retain(|label, _| live.contains(label));
    for label in state.labels() {
        if !live.contains(&label) {
            state.forget(&label);
        }
    }
}

fn report(label: &str, decision: &ReloadDecision, silent_for: Duration, futile_reloads: u32) {
    if decision.futile {
        eprintln!(
            "[Tauri] WKWebView heartbeat for {label} silent for {:.0}s and the page has not beaten \
             ONCE since the last reload ({futile_reloads} futile reloads). Reloading anyway, then \
             backing off to {:.0}s. A reload that never restores the heartbeat means the page is \
             running but cannot reach us: check the engine log for [Client/ipc] lines, and check \
             for \"not allowed by ACL\" rejections.",
            silent_for.as_secs_f64(),
            decision.next_threshold.as_secs_f64(),
        );
    } else {
        eprintln!(
            "[Tauri] WKWebView heartbeat timeout for {label} ({:.0}s) while the window is on \
             screen: reloading",
            silent_for.as_secs_f64()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG_SILENCE: Duration = Duration::from_secs(60 * 60);

    fn beats(count: u64) -> Pulse {
        Pulse { count, page: None }
    }

    fn doc(count: u64, page: &str) -> Pulse {
        Pulse {
            count,
            page: Some(page.to_string()),
        }
    }

    fn past_threshold(watchdog: &ReloadWatchdog) -> Duration {
        reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1)
    }

    fn reload_of(verdict: Verdict) -> ReloadDecision {
        match verdict {
            Verdict::Reload(decision) => decision,
            other => panic!("expected a reload, got {other:?}"),
        }
    }

    #[test]
    fn the_watchdog_reloads_only_past_the_timeout() {
        let mut watchdog = ReloadWatchdog::default();
        assert_eq!(
            watchdog.on_tick(true, Duration::from_secs(59), beats(100)),
            Verdict::Watching
        );
        assert_eq!(
            watchdog.on_tick(true, HEARTBEAT_TIMEOUT, beats(100)),
            Verdict::Watching
        );
        assert_eq!(
            watchdog.on_tick(true, Duration::from_secs(61), beats(100)),
            Verdict::Reload(ReloadDecision {
                futile: false,
                next_threshold: HEARTBEAT_TIMEOUT,
            })
        );
    }

    #[test]
    fn a_parked_window_is_never_reloaded_however_long_it_is_silent() {
        let mut watchdog = ReloadWatchdog::default();
        for _ in 0..20 {
            assert_eq!(
                watchdog.on_tick(false, LONG_SILENCE, beats(0)),
                Verdict::Parked
            );
        }
    }

    #[test]
    fn a_park_clears_a_backoff_the_page_earned_before_it() {
        let mut watchdog = ReloadWatchdog::default();
        for _ in 0..4 {
            let silent = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
            watchdog.on_tick(true, silent, beats(0));
        }
        assert!(watchdog.futile_reloads > 0, "expected to be backed off");
        assert_eq!(
            watchdog.on_tick(false, LONG_SILENCE, beats(0)),
            Verdict::Parked
        );
        assert_eq!(watchdog.futile_reloads, 0);
        let decision = reload_of(watchdog.on_tick(true, Duration::from_secs(61), beats(0)));
        assert!(!decision.futile);
        assert_eq!(decision.next_threshold, HEARTBEAT_TIMEOUT);
    }

    #[test]
    fn two_windows_are_judged_independently() {
        let mut healthy = ReloadWatchdog::default();
        let mut broken = ReloadWatchdog::default();
        for _ in 0..3 {
            assert_eq!(
                healthy.on_tick(true, Duration::from_secs(5), beats(42)),
                Verdict::Watching
            );
            let silent = reload_threshold(broken.futile_reloads) + Duration::from_secs(1);
            assert!(matches!(
                broken.on_tick(true, silent, beats(0)),
                Verdict::Reload(_)
            ));
        }
        assert_eq!(healthy.futile_reloads, 0);
        assert!(broken.futile_reloads > 0);
    }

    #[test]
    fn a_reload_that_restores_the_heartbeat_keeps_the_base_interval() {
        let mut watchdog = ReloadWatchdog::default();
        for count in [10_u64, 25, 40] {
            let decision = reload_of(watchdog.on_tick(
                true,
                HEARTBEAT_TIMEOUT + Duration::from_secs(1),
                beats(count),
            ));
            assert!(!decision.futile);
            assert_eq!(decision.next_threshold, HEARTBEAT_TIMEOUT);
        }
    }

    #[test]
    fn reloads_that_never_restore_the_heartbeat_back_off_instead_of_thrashing() {
        let mut watchdog = ReloadWatchdog::default();
        let mut thresholds = Vec::new();
        for _ in 0..8 {
            let silent_for = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
            thresholds.push(reload_of(watchdog.on_tick(true, silent_for, beats(0))).next_threshold);
        }
        assert_eq!(
            thresholds,
            vec![
                HEARTBEAT_TIMEOUT,
                HEARTBEAT_TIMEOUT * 2,
                HEARTBEAT_TIMEOUT * 4,
                HEARTBEAT_TIMEOUT * 8,
                HEARTBEAT_TIMEOUT * 16,
                HEARTBEAT_TIMEOUT * 32,
                HEARTBEAT_TIMEOUT * 32,
                HEARTBEAT_TIMEOUT * 32,
            ]
        );
    }

    #[test]
    fn the_backoff_resets_as_soon_as_the_page_beats_again() {
        let mut watchdog = ReloadWatchdog::default();
        for _ in 0..4 {
            let silent_for = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
            watchdog.on_tick(true, silent_for, beats(0));
        }
        assert!(watchdog.futile_reloads > 0, "expected to be backed off");
        let silent_for = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
        let decision = reload_of(watchdog.on_tick(true, silent_for, beats(1)));
        assert!(!decision.futile);
        assert_eq!(decision.next_threshold, HEARTBEAT_TIMEOUT);
        assert_eq!(watchdog.futile_reloads, 0);
    }

    #[test]
    fn the_backoff_never_stops_retrying() {
        assert_eq!(
            reload_threshold(u32::MAX),
            reload_threshold(MAX_RELOAD_BACKOFF_DOUBLINGS)
        );
        assert!(reload_threshold(u32::MAX) <= Duration::from_secs(60 * 60));
    }

    #[test]
    fn one_windows_heartbeat_does_not_speak_for_another() {
        let beats = Heartbeats::default();
        beats.record("main", None, None);
        beats.record("main", None, None);
        let main = beats.read("main");
        let other = beats.read("window-0");
        assert_eq!(main.pulse.count, 2);
        assert_eq!(
            other.pulse.count, 0,
            "a window that never beat must read zero"
        );
        assert!(main.silent_for < Duration::from_secs(1));
        assert!(other.silent_for < Duration::from_secs(1));
    }

    #[test]
    fn a_window_that_is_gone_takes_its_heartbeat_with_it() {
        let beats = Heartbeats::default();
        beats.record("window-7", None, None);
        assert_eq!(beats.labels(), vec!["window-7".to_string()]);
        beats.forget("window-7");
        assert!(beats.labels().is_empty());
    }

    #[test]
    fn a_stale_beat_from_the_old_document_does_not_make_a_reload_succeed() {
        let mut watchdog = ReloadWatchdog::default();
        let first = reload_of(watchdog.on_tick(
            true,
            HEARTBEAT_TIMEOUT + Duration::from_secs(1),
            doc(10, "old"),
        ));
        assert!(!first.futile);
        let second = reload_of(watchdog.on_tick(true, past_threshold(&watchdog), doc(11, "old")));
        assert!(second.futile);
        assert_eq!(second.next_threshold, HEARTBEAT_TIMEOUT * 2);
    }

    #[test]
    fn a_beat_from_a_new_document_resets_the_backoff() {
        let mut watchdog = ReloadWatchdog::default();
        for _ in 0..4 {
            let silent = past_threshold(&watchdog);
            watchdog.on_tick(true, silent, doc(3, "old"));
        }
        assert!(watchdog.futile_reloads > 0, "expected to be backed off");
        let decision = reload_of(watchdog.on_tick(true, past_threshold(&watchdog), doc(4, "new")));
        assert!(!decision.futile);
        assert_eq!(decision.next_threshold, HEARTBEAT_TIMEOUT);
    }

    #[test]
    fn a_page_without_an_id_falls_back_to_the_beat_count() {
        let mut watchdog = ReloadWatchdog::default();
        reload_of(watchdog.on_tick(true, LONG_SILENCE, beats(5)));
        assert!(reload_of(watchdog.on_tick(true, LONG_SILENCE, beats(5))).futile);
        assert!(!reload_of(watchdog.on_tick(true, LONG_SILENCE, beats(6))).futile);
    }

    #[test]
    fn a_beat_records_the_document_and_what_the_page_reported() {
        let beats = Heartbeats::default();
        beats.record("main", Some("doc-1".into()), Some(false));
        let hidden = beats.read("main");
        assert_eq!(hidden.pulse.page.as_deref(), Some("doc-1"));
        assert!(hidden.page_reported_hidden);
        beats.record("main", Some("doc-1".into()), Some(true));
        assert!(!beats.read("main").page_reported_hidden);
    }

    #[test]
    fn a_page_that_reports_no_visibility_counts_as_visible() {
        let beats = Heartbeats::default();
        beats.record("main", None, None);
        let reading = beats.read("main");
        assert!(!reading.page_reported_hidden);
        assert_eq!(reading.pulse.page, None);
    }
}
