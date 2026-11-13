//! WKWebView crash recovery, per app window.
//!
//! macOS can terminate a WKWebView's content process under memory pressure, and
//! can suspend it when it reads the window as off screen. Either way the page
//! stops, with no event to tell us. The page heartbeats instead, and silence is
//! the signal.
//!
//! Silence alone is not a fault. A window the client parked in the tray is
//! SUPPOSED to go quiet. Reloading it throws away the page state ADR 0141
//! promises to keep. So the watchdog asks `window_screen` whether the window is
//! on screen, and only acts when it is. ADR 0180 has the reasoning.
//!
//! A covered window is on screen, yet WebKit suspends its page. The page reports
//! `document.visibilityState` in its beat, and a hidden page in a window that is
//! not key counts as parked too. ADR 0660 has the incident.
//!
//! A reload that brings back no heartbeat fixed nothing, so the interval backs
//! off and says so. A silent reload loop hides the real fault for months. The
//! test is a beat from a NEW document: a resumed process lets the old document
//! beat once before the reload commits, and that beat proves nothing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::Manager;

/// The JS heartbeat, per webview label.
///
/// **Per label, not per app.** Every window's page beats, and one shared slot
/// meant a healthy window spoke for a dead one. With two windows open the
/// watchdog could not see either fail.
#[derive(Default)]
pub(crate) struct Heartbeats {
    beats: Mutex<HashMap<String, Beat>>,
}

/// What the watchdog knows about the beats it has received.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Pulse {
    /// Monotonic count of heartbeats received.
    ///
    /// The watchdog resets the timestamp itself on every reload. So the
    /// timestamp alone cannot tell a page that came back and died again from
    /// one that has never beaten at all. The count can, and that distinction is
    /// what stops a pointless reload from repeating forever.
    count: u64,
    /// The document that sent the latest beat. `None` for a page too old to say.
    page: Option<String>,
}

impl Pulse {
    /// Did a reload that left `earlier` behind bring anything back?
    ///
    /// A page id is exact: the same id means the same document is still the
    /// one beating. Without one on both sides the count is the best evidence.
    fn is_unchanged_since(&self, earlier: &Pulse) -> bool {
        match (&self.page, &earlier.page) {
            (Some(now), Some(then)) => now == then,
            _ => self.count == earlier.count,
        }
    }
}

/// One page's liveness.
struct Beat {
    /// When the most recent heartbeat arrived.
    at: Instant,
    pulse: Pulse,
    /// What the page last said about its own visibility. `None` for a page too
    /// old to say, which counts as visible.
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

/// One window's silence and what its page last said.
struct Reading {
    silent_for: Duration,
    pulse: Pulse,
    /// WebKit told the page it is hidden, so it may suspend the process.
    page_reported_hidden: bool,
}

impl Heartbeats {
    /// A page said it is alive, and optionally which document it is and whether
    /// WebKit shows it.
    fn record(&self, label: &str, page: Option<String>, visible: Option<bool>) {
        let mut beats = self.beats.lock().unwrap();
        let beat = beats.entry(label.to_string()).or_insert_with(Beat::new);
        beat.at = Instant::now();
        beat.pulse.count += 1;
        beat.pulse.page = page;
        beat.visible = visible;
    }

    /// How long this page has been silent, and what it last reported.
    /// Starts the clock for a page nobody has heard from yet, so the warm-up is
    /// measured from first sight rather than from process start.
    fn read(&self, label: &str) -> Reading {
        let mut beats = self.beats.lock().unwrap();
        let beat = beats.entry(label.to_string()).or_insert_with(Beat::new);
        Reading {
            silent_for: beat.at.elapsed(),
            pulse: beat.pulse.clone(),
            page_reported_hidden: beat.visible == Some(false),
        }
    }

    /// Restart this page's silence clock, after a reload or a park.
    fn reset_clock(&self, label: &str) {
        if let Some(beat) = self.beats.lock().unwrap().get_mut(label) {
            beat.at = Instant::now();
        }
    }

    /// Every label the map holds, for the sweep that drops dead windows.
    fn labels(&self) -> Vec<String> {
        self.beats.lock().unwrap().keys().cloned().collect()
    }

    /// Drop a window that is gone, so the map cannot grow one entry per window
    /// the user ever opened.
    fn forget(&self, label: &str) {
        self.beats.lock().unwrap().remove(label);
    }
}

/// How long the JS heartbeat may go silent, on an ON-SCREEN window, before the
/// watchdog treats the content process as lost and reloads it. The page
/// heartbeats every 15s, so 60s is four missed beats.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(60);

/// How often the watchdog re-checks. Well under [`HEARTBEAT_TIMEOUT`] so a
/// genuine crash is caught promptly.
const WATCHDOG_TICK: Duration = Duration::from_secs(15);

/// How long the watchdog lets the webview load before its first tick.
const WARMUP: Duration = Duration::from_secs(30);

/// Cap on the backoff doublings in [`reload_threshold`]: 60s << 5 ≈ 32 minutes.
/// Bounded rather than unbounded so a page that recovers on its own (say the
/// gateway finally comes up) is still noticed within a useful window.
const MAX_RELOAD_BACKOFF_DOUBLINGS: u32 = 5;

/// How long the heartbeat may go silent before the next reload, given how many
/// consecutive reloads have already failed to bring it back.
///
/// A reload that produces no heartbeat did not fix anything. Repeating it on the
/// base interval is how a broken IPC bridge becomes a silent reload every minute
/// for weeks. Backing off rather than giving up kills the thrash while still
/// recovering if the cause was temporary.
fn reload_threshold(futile_reloads: u32) -> Duration {
    HEARTBEAT_TIMEOUT * 2u32.pow(futile_reloads.min(MAX_RELOAD_BACKOFF_DOUBLINGS))
}

/// What the watchdog decided about one window on one tick.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// The window is off screen, so silence is expected and the page is left
    /// alone. The caller restarts the clock, so a window that comes back is
    /// judged from its RETURN rather than from its park. Without that, a window
    /// parked overnight is reloaded the instant the user reopens it. A reload is
    /// the one thing a park avoids.
    Parked,
    /// On screen, and either beating or not yet silent long enough.
    Watching,
    /// On screen and silent past the threshold.
    Reload(ReloadDecision),
}

/// What a reload is expected to achieve, and when the next one may follow.
#[derive(Debug, PartialEq, Eq)]
struct ReloadDecision {
    /// The previous reload produced no heartbeat at all, so this one is very
    /// unlikely to help either. Something other than a content-process crash is
    /// wrong, an ACL-rejected IPC bridge for instance.
    futile: bool,
    /// How long the heartbeat may now go silent before the watchdog tries again.
    next_threshold: Duration,
}

/// The watchdog's state machine for ONE window, kept pure so the escalation is
/// unit-testable without a webview or a 32-minute wall clock.
#[derive(Debug, Default)]
struct ReloadWatchdog {
    /// What the beats looked like at the last reload; `None` before the first
    /// one, so the first reload is never judged futile.
    pulse_at_last_reload: Option<Pulse>,
    /// Consecutive reloads after which the page still never beat.
    futile_reloads: u32,
}

impl ReloadWatchdog {
    /// One watchdog tick for one window. The caller must restart that window's
    /// silence clock for anything but [`Verdict::Watching`].
    fn on_tick(&mut self, on_screen: bool, silent_for: Duration, pulse: Pulse) -> Verdict {
        if !on_screen {
            // A park is a fresh start. Carrying the backoff across one would
            // let a night in the tray decide how fast a real crash is caught.
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

/// The page telling us it is still alive. Registered as an app command.
///
/// It takes the calling WEBVIEW so the beat is attributed to one window. An
/// `AppHandle` cannot say which page beat, and that is how one healthy window
/// hid every other window's silence.
///
/// `page` and `visible` are optional so an older page, such as a remote gateway
/// on an earlier release, still beats with no arguments.
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

/// Start watching every app window. Its own thread, running for the life of the
/// process.
///
/// By webview, not webview window, per ADR 0140. Reading a URL and navigating
/// are page operations, and the blind flavour found no window at all while a
/// preview was open. Recovery was therefore off in exactly the case that needs
/// it, a previewed remote page taking the content process down.
pub(crate) fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        // Let the webview load before the watchdog starts.
        std::thread::sleep(WARMUP);
        let mut watchdogs: HashMap<String, ReloadWatchdog> = HashMap::new();
        loop {
            std::thread::sleep(WATCHDOG_TICK);
            tick(&app, &mut watchdogs);
        }
    });
}

/// One pass over every app window.
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
        // An unreadable focus counts as key, so the window stays watched.
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
                // Reset the clock so the next threshold is measured from this
                // reload rather than from the last heartbeat.
                state.reset_clock(&label);
            }
        }
    }
    // Windows that have gone away take their state with them.
    watchdogs.retain(|label, _| live.contains(label));
    for label in state.labels() {
        if !live.contains(&label) {
            state.forget(&label);
        }
    }
}

/// Say what is about to happen and why, before it happens.
///
/// It names the window and how long its page has been quiet. The freeze this
/// watchdog was rebuilt for had to be diagnosed from `log show` against a WebKit
/// internal. The client's own log said nothing.
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

    /// Silent well past any threshold, so only the on-screen decision is under
    /// test.
    const LONG_SILENCE: Duration = Duration::from_secs(60 * 60);

    /// Beats from a page too old to send a document id.
    fn beats(count: u64) -> Pulse {
        Pulse { count, page: None }
    }

    /// Beats from a page that names its document.
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
        // Below and at the threshold: no reload (15s heartbeat cadence).
        assert_eq!(
            watchdog.on_tick(true, Duration::from_secs(59), beats(100)),
            Verdict::Watching
        );
        assert_eq!(
            watchdog.on_tick(true, HEARTBEAT_TIMEOUT, beats(100)),
            Verdict::Watching
        );
        // Strictly past the timeout: reload. The first one is never futile,
        // since there is no earlier reload for it to have failed to improve on.
        assert_eq!(
            watchdog.on_tick(true, Duration::from_secs(61), beats(100)),
            Verdict::Reload(ReloadDecision {
                futile: false,
                next_threshold: HEARTBEAT_TIMEOUT,
            })
        );
    }

    /// The bug this rebuild exists for. A window the client parked is SUPPOSED
    /// to go quiet, and a reload throws away the page state that makes the
    /// reopen instant (ADR 0141). Before this, a client parked overnight
    /// reloaded its pages all night.
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

    /// A park must not leave the backoff behind it. Coming back from the tray is
    /// a fresh start, so the first real crash after one is caught in 60s rather
    /// than in 32 minutes.
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

    /// Each window keeps its own state, which is the other half of the
    /// per-window heartbeat. One window's crash must not back the other off, and
    /// one window's health must not speak for the other.
    #[test]
    fn two_windows_are_judged_independently() {
        let mut healthy = ReloadWatchdog::default();
        let mut broken = ReloadWatchdog::default();
        for _ in 0..3 {
            // The healthy one keeps beating, so it never crosses the threshold.
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
        // Genuine content-process crash: reload, page comes back and beats, and
        // some time later it crashes again. Each recovery keeps the fast 60s
        // interval, because reloading is demonstrably working.
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
        // A rejected `invoke`: the page loads and runs, but the count NEVER
        // advances. Without the backoff this reloads once a minute forever and
        // says nothing new.
        let mut watchdog = ReloadWatchdog::default();
        let mut thresholds = Vec::new();
        for _ in 0..8 {
            // Always just past whatever the current threshold is.
            let silent_for = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
            thresholds.push(reload_of(watchdog.on_tick(true, silent_for, beats(0))).next_threshold);
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
        // Backing off must not become permanent. The reloads may have been
        // futile only because the gateway was down, and then the page finally
        // loads and beats. Full-speed crash recovery has to come straight back.
        let mut watchdog = ReloadWatchdog::default();
        for _ in 0..4 {
            let silent_for = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
            watchdog.on_tick(true, silent_for, beats(0));
        }
        assert!(watchdog.futile_reloads > 0, "expected to be backed off");
        // One heartbeat arrives, then silence again.
        let silent_for = reload_threshold(watchdog.futile_reloads) + Duration::from_secs(1);
        let decision = reload_of(watchdog.on_tick(true, silent_for, beats(1)));
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

    /// One window's page beating must leave every other window's silence
    /// visible. A single shared slot is what hid the reported freeze: the
    /// watchdog saw a healthy count and never looked at the frozen window.
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
        // The silent window's clock starts at FIRST SIGHT, so its warm-up is not
        // measured from process start.
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

    /// The incident: `loadRequest` resumed the suspended process, and the OLD
    /// document sent one overdue beat before the new one committed. The count
    /// moved, so every reload looked like a success and the backoff never began.
    #[test]
    fn a_stale_beat_from_the_old_document_does_not_make_a_reload_succeed() {
        let mut watchdog = ReloadWatchdog::default();
        let first = reload_of(watchdog.on_tick(
            true,
            HEARTBEAT_TIMEOUT + Duration::from_secs(1),
            doc(10, "old"),
        ));
        assert!(!first.futile);
        // The old document beat once more. The new one never did.
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

    /// A remote gateway on an older release sends no document id. Its futility
    /// stays judged by the count, as before.
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

    /// An old page sends neither argument, and must still be watched.
    #[test]
    fn a_page_that_reports_no_visibility_counts_as_visible() {
        let beats = Heartbeats::default();
        beats.record("main", None, None);
        let reading = beats.read("main");
        assert!(!reading.page_reported_hidden);
        assert_eq!(reading.pulse.page, None);
    }
}
