//! WKWebView crash recovery for the `main` window.
//!
//! macOS can terminate a WKWebView's content process under memory pressure,
//! leaving a white screen with no event to tell us. The page heartbeats
//! instead, and a heartbeat that stops arriving is the signal to reload.
//!
//! A reload that brings back no heartbeat fixed nothing, so the interval backs
//! off and says so. A silent reload loop hides the real fault for months.

use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use std::time::Instant;

use tauri::Manager;

/// Tracks the JS heartbeat. The JS side calls [`heartbeat`] every 15s; if we
/// don't hear from it for [`HEARTBEAT_TIMEOUT`], the watchdog reloads.
pub(crate) struct LastHeartbeat {
    /// When the most recent heartbeat arrived.
    at: Mutex<Instant>,
    /// Monotonic count of heartbeats received.
    ///
    /// The watchdog resets the timestamp itself on every reload. So the
    /// timestamp alone cannot tell a page that came back and died again from
    /// one that has never beaten at all. The count can, and that distinction is
    /// what stops a pointless reload from repeating forever.
    count: AtomicU64,
}

impl Default for LastHeartbeat {
    fn default() -> Self {
        Self {
            at: Mutex::new(Instant::now()),
            count: AtomicU64::new(0),
        }
    }
}

/// How long the JS heartbeat may go silent before the watchdog treats the
/// WKWebView content process as crashed and reloads it. The page heartbeats
/// every 15s, so 60s is four missed beats.
const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How often the watchdog re-checks. Well under [`HEARTBEAT_TIMEOUT`] so a
/// genuine crash is caught promptly.
const WATCHDOG_TICK: std::time::Duration = std::time::Duration::from_secs(15);

/// How long the watchdog lets the webview load before its first tick.
const WARMUP: std::time::Duration = std::time::Duration::from_secs(30);

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
fn reload_threshold(futile_reloads: u32) -> std::time::Duration {
    HEARTBEAT_TIMEOUT * 2u32.pow(futile_reloads.min(MAX_RELOAD_BACKOFF_DOUBLINGS))
}

/// What the watchdog decided to do on one tick.
#[derive(Debug, PartialEq, Eq)]
struct ReloadDecision {
    /// The previous reload produced no heartbeat at all, so this one is very
    /// unlikely to help either. Something other than a content-process crash is
    /// wrong, an ACL-rejected IPC bridge for instance.
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

/// The page telling us it is still alive. Registered as an app command.
#[tauri::command]
pub(crate) fn heartbeat(app: tauri::AppHandle) {
    let state = app.state::<LastHeartbeat>();
    *state.at.lock().unwrap() = Instant::now();
    state
        .count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Start watching `main`. Its own thread, running for the life of the process.
///
/// By webview, not webview window, per ADR 0140. Reading a URL and navigating
/// are page operations, and the blind flavour found no window at all while a
/// preview was open. Recovery was therefore off in exactly the case that needs
/// it, a previewed remote page taking the content process down.
pub(crate) fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        // Let the webview load before the watchdog starts.
        std::thread::sleep(WARMUP);
        let mut watchdog = ReloadWatchdog::default();
        loop {
            std::thread::sleep(WATCHDOG_TICK);
            let Some(webview) = app.get_webview(crate::app_window::MAIN_WINDOW_LABEL) else {
                continue;
            };
            let state = app.state::<LastHeartbeat>();
            let silent_for = state.at.lock().unwrap().elapsed();
            let heartbeats = state.count.load(std::sync::atomic::Ordering::Relaxed);
            let Some(decision) = watchdog.on_tick(silent_for, heartbeats) else {
                continue;
            };
            report(&decision, silent_for, watchdog.futile_reloads);
            if let Ok(url) = webview.url() {
                let _ = webview.navigate(url);
            }
            // Reset the clock so the next threshold is measured from this
            // reload rather than from the last heartbeat.
            *state.at.lock().unwrap() = Instant::now();
        }
    });
}

/// Say what is about to happen and why, before it happens.
fn report(decision: &ReloadDecision, silent_for: std::time::Duration, futile_reloads: u32) {
    if decision.futile {
        eprintln!(
            "[Tauri] WKWebView heartbeat silent for {:.0}s and the page has not beaten ONCE since \
             the last reload ({futile_reloads} futile reloads). Reloading anyway, then backing \
             off to {:.0}s. A reload that never restores the heartbeat means the page is running \
             but cannot reach us: check the engine log for [Client/ipc] lines, and check for \
             \"not allowed by ACL\" rejections.",
            silent_for.as_secs_f64(),
            decision.next_threshold.as_secs_f64(),
        );
    } else {
        eprintln!(
            "[Tauri] WKWebView heartbeat timeout ({:.0}s): reloading",
            silent_for.as_secs_f64()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        // Backing off must not become permanent. The reloads may have been
        // futile only because the gateway was down, and then the page finally
        // loads and beats. Full-speed crash recovery has to come straight back.
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
}
