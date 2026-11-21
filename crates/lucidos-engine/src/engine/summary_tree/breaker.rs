//! The compactor's circuit breaker: one per model on a provider route.
//!
//! A provider that is down fails every call at once. Without a breaker, each
//! owed scope would call it again every [`RETRY`]. The breaker trips after
//! [`TRIP_AFTER`] failed calls in a row, and any success resets the streak.
//!
//! While open, a node waits before it takes a lane permit. When the wait ends,
//! exactly one call goes as the probe and the rest keep waiting. A successful
//! call closes the breaker and wakes every waiter. A failed probe doubles the
//! wait, up to [`MAX_WAIT`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use super::RETRY;

/// Failed calls in a row that open a breaker.
pub(crate) const TRIP_AFTER: u32 = 3;

/// Longest wait between probes. Short on purpose: [`RETRY`] explains why a
/// long back-off starves whatever waits on summaries.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(300);

/// The first wait after a trip, and the longest wait between probes.
#[derive(Clone, Copy)]
pub(crate) struct Waits {
    first: Duration,
    longest: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Self {
            first: RETRY,
            longest: MAX_WAIT,
        }
    }
}

impl Waits {
    /// Short waits, so a test need not sit through the real ones.
    #[cfg(test)]
    pub(crate) fn new(first: Duration, longest: Duration) -> Self {
        Self { first, longest }
    }
}

/// Every breaker the compactor has called through, by route and model.
#[derive(Default)]
pub(crate) struct Breakers {
    waits: Waits,
    breakers: Mutex<HashMap<(String, String), Arc<Breaker>>>,
}

impl Breakers {
    #[cfg(test)]
    pub(crate) fn with_waits(waits: Waits) -> Self {
        Self {
            waits,
            ..Self::default()
        }
    }

    pub(crate) fn breaker(&self, lane: &str, model: &str) -> Arc<Breaker> {
        self.breakers
            .lock()
            .unwrap()
            .entry((lane.to_string(), model.to_string()))
            .or_insert_with(|| Arc::new(Breaker::new(self.waits)))
            .clone()
    }
}

#[derive(Default)]
pub(crate) struct Breaker {
    waits: Waits,
    state: Mutex<State>,
    changed: Notify,
}

#[derive(Default)]
struct State {
    phase: Phase,
    /// This outage already sent its notice. A close re-arms it.
    noticed: bool,
}

enum Phase {
    Closed {
        failed_in_a_row: u32,
    },
    /// Calls wait until `until`, then one goes as the probe.
    Open {
        until: Instant,
        wait: Duration,
    },
    /// The probe is in flight. The next failure waits twice `wait`.
    Probing {
        wait: Duration,
    },
}

impl Default for Phase {
    fn default() -> Self {
        Self::Closed { failed_in_a_row: 0 }
    }
}

/// What a call's verdict did to the breaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transition {
    Unchanged,
    /// The streak reached [`TRIP_AFTER`].
    Opened {
        wait: Duration,
    },
    /// The probe failed. `notice` is set once per outage, the first time the
    /// wait reaches [`MAX_WAIT`].
    Backoff {
        wait: Duration,
        notice: bool,
    },
    Closed,
}

impl Breaker {
    fn new(waits: Waits) -> Self {
        Self {
            waits,
            ..Self::default()
        }
    }

    /// Wait until a call may go. The admission says whether it is the probe.
    pub(crate) async fn admit(self: &Arc<Self>) -> Admission {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let until = {
                let mut state = self.state.lock().unwrap();
                match state.phase {
                    Phase::Closed { .. } => return self.admission(false),
                    Phase::Open { until, wait } if Instant::now() >= until => {
                        state.phase = Phase::Probing { wait };
                        return self.admission(true);
                    }
                    Phase::Open { until, .. } => Some(until),
                    Phase::Probing { .. } => None,
                }
            };
            match until {
                Some(until) => {
                    tokio::select! {
                        () = &mut changed => {}
                        () = tokio::time::sleep_until(until) => {}
                    }
                }
                None => changed.await,
            }
        }
    }

    /// Whether calls wait: the breaker is open or probing.
    pub(crate) fn is_open(&self) -> bool {
        !matches!(self.state.lock().unwrap().phase, Phase::Closed { .. })
    }

    fn admission(self: &Arc<Self>, probe: bool) -> Admission {
        Admission {
            breaker: self.clone(),
            probe,
            settled: false,
        }
    }

    fn settle_success(&self) -> Transition {
        let mut state = self.state.lock().unwrap();
        let was_open = !matches!(state.phase, Phase::Closed { .. });
        state.phase = Phase::default();
        state.noticed = false;
        if was_open {
            self.changed.notify_waiters();
            Transition::Closed
        } else {
            Transition::Unchanged
        }
    }

    fn settle_failure(&self, probe: bool) -> Transition {
        let mut state = self.state.lock().unwrap();
        match state.phase {
            Phase::Closed { failed_in_a_row } => {
                let failed_in_a_row = failed_in_a_row + 1;
                if failed_in_a_row < TRIP_AFTER {
                    state.phase = Phase::Closed { failed_in_a_row };
                    return Transition::Unchanged;
                }
                let wait = self.waits.first;
                state.phase = Phase::Open {
                    until: Instant::now() + wait,
                    wait,
                };
                Transition::Opened { wait }
            }
            Phase::Probing { wait } if probe => {
                let wait = (wait * 2).min(self.waits.longest);
                state.phase = Phase::Open {
                    until: Instant::now() + wait,
                    wait,
                };
                // Waiters parked on the probe now wait for its next wait.
                self.changed.notify_waiters();
                let notice = wait == self.waits.longest && !state.noticed;
                state.noticed |= notice;
                Transition::Backoff { wait, notice }
            }
            // A call that started before the trip says nothing new, and only
            // the probe's own failure judges the probe.
            Phase::Open { .. } | Phase::Probing { .. } => Transition::Unchanged,
        }
    }

    /// A probe that ended with no verdict hands the probe on: the wait is
    /// over, so the next waiter probes.
    fn hand_on_probe(&self) {
        let mut state = self.state.lock().unwrap();
        if let Phase::Probing { wait } = state.phase {
            state.phase = Phase::Open {
                until: Instant::now(),
                wait,
            };
            self.changed.notify_waiters();
        }
    }
}

/// One call's leave to go. Report how it went, or drop it to say nothing.
pub(crate) struct Admission {
    breaker: Arc<Breaker>,
    probe: bool,
    settled: bool,
}

impl Admission {
    pub(crate) fn is_probe(&self) -> bool {
        self.probe
    }

    /// Whether the call may still go. A call admitted while the breaker was
    /// closed loses its leave once the breaker opens.
    pub(crate) fn still_admitted(&self) -> bool {
        self.probe || !self.breaker.is_open()
    }

    /// The provider answered, whatever the answer held.
    pub(crate) fn succeeded(mut self) -> Transition {
        self.settled = true;
        self.breaker.settle_success()
    }

    /// The model call itself failed.
    pub(crate) fn failed(mut self) -> Transition {
        self.settled = true;
        self.breaker.settle_failure(self.probe)
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if self.probe && !self.settled {
            self.breaker.hand_on_probe();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn waiting<F: std::future::Future>(f: &mut std::pin::Pin<&mut F>) -> bool {
        tokio::task::yield_now().await;
        futures::poll!(f.as_mut()).is_pending()
    }

    async fn trip(breaker: &Arc<Breaker>) {
        for _ in 0..TRIP_AFTER {
            breaker.admit().await.failed();
        }
    }

    /// Run the open breaker to its probe and fail it.
    async fn fail_probe(breaker: &Arc<Breaker>, wait: Duration) -> Transition {
        tokio::time::advance(wait).await;
        let probe = breaker.admit().await;
        assert!(probe.probe);
        probe.failed()
    }

    #[tokio::test(start_paused = true)]
    async fn failures_in_a_row_open_the_breaker() {
        let breaker = Arc::new(Breaker::default());
        for _ in 0..TRIP_AFTER - 1 {
            assert_eq!(breaker.admit().await.failed(), Transition::Unchanged);
            assert!(!breaker.is_open());
        }
        assert_eq!(
            breaker.admit().await.failed(),
            Transition::Opened { wait: RETRY }
        );
        assert!(breaker.is_open());
    }

    #[tokio::test(start_paused = true)]
    async fn a_success_resets_the_streak() {
        let breaker = Arc::new(Breaker::default());
        for _ in 0..10 {
            breaker.admit().await.failed();
            breaker.admit().await.failed();
            assert_eq!(breaker.admit().await.succeeded(), Transition::Unchanged);
        }
        assert!(!breaker.is_open(), "failures between successes never trip");
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_breaker_lets_one_probe_through_after_the_wait() {
        let breaker = Arc::new(Breaker::default());
        trip(&breaker).await;

        let first = breaker.admit();
        let second = breaker.admit();
        tokio::pin!(first, second);
        assert!(waiting(&mut first).await);
        assert!(waiting(&mut second).await);

        tokio::time::advance(RETRY).await;
        let probe = futures::future::select(first, second).await;
        let (probe, mut other) = match probe {
            futures::future::Either::Left((p, o)) => (p, o),
            futures::future::Either::Right((p, o)) => (p, o),
        };
        assert!(probe.probe);
        assert!(waiting(&mut other).await, "only one call probes");
        probe.succeeded();
        assert!(!other.await.probe, "a success closes the breaker");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_probe_doubles_the_wait_up_to_the_cap() {
        let breaker = Arc::new(Breaker::default());
        trip(&breaker).await;
        let mut wait = RETRY;
        while wait < MAX_WAIT {
            let next = (wait * 2).min(MAX_WAIT);
            let transition = fail_probe(&breaker, wait).await;
            assert!(matches!(
                transition,
                Transition::Backoff { wait: w, .. } if w == next
            ));
            wait = next;
        }
        assert_eq!(wait, MAX_WAIT);
        let transition = fail_probe(&breaker, MAX_WAIT).await;
        assert_eq!(
            transition,
            Transition::Backoff {
                wait: MAX_WAIT,
                notice: false
            },
            "the cap sends its notice once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_notice_comes_once_per_outage() {
        let breaker = Arc::new(Breaker::default());
        let mut notices = 0;
        for _ in 0..2 {
            trip(&breaker).await;
            let mut wait = RETRY;
            for _ in 0..8 {
                if let Transition::Backoff { notice, .. } = fail_probe(&breaker, wait).await {
                    notices += usize::from(notice);
                }
                wait = (wait * 2).min(MAX_WAIT);
            }
            tokio::time::advance(wait).await;
            assert_eq!(breaker.admit().await.succeeded(), Transition::Closed);
        }
        assert_eq!(notices, 2, "one per outage, and a close re-arms it");
    }

    #[tokio::test(start_paused = true)]
    async fn a_close_resets_the_wait_and_wakes_every_waiter() {
        let breaker = Arc::new(Breaker::default());
        trip(&breaker).await;
        fail_probe(&breaker, RETRY).await;

        let mut waiters: Vec<_> = (0..3)
            .map(|_| {
                let breaker = breaker.clone();
                tokio::spawn(async move { breaker.admit().await })
            })
            .collect();
        tokio::task::yield_now().await;
        tokio::time::advance(RETRY * 2).await;
        tokio::task::yield_now().await;
        let probing = waiters.iter().position(|w| w.is_finished()).unwrap();
        let probe = waiters.remove(probing).await.unwrap();
        assert!(probe.probe);
        assert!(
            waiters.iter().all(|w| !w.is_finished()),
            "the other waiters park while the probe is out"
        );

        assert_eq!(probe.succeeded(), Transition::Closed);
        for waiter in waiters {
            assert!(!waiter.await.unwrap().probe, "no waiter probes any more");
        }

        trip(&breaker).await;
        assert!(matches!(
            fail_probe(&breaker, RETRY).await,
            Transition::Backoff { wait, .. } if wait == RETRY * 2
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_waiter_parked_on_a_failed_probe_probes_after_the_next_wait() {
        let breaker = Arc::new(Breaker::default());
        trip(&breaker).await;
        tokio::time::advance(RETRY).await;
        let probe = breaker.admit().await;

        let waiter = breaker.admit();
        tokio::pin!(waiter);
        assert!(waiting(&mut waiter).await);
        probe.failed();
        assert!(waiting(&mut waiter).await, "the wait doubled");

        tokio::time::advance(RETRY * 2).await;
        assert!(waiter.await.probe);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_probe_hands_the_probe_on() {
        let breaker = Arc::new(Breaker::default());
        trip(&breaker).await;
        tokio::time::advance(RETRY).await;
        let probe = breaker.admit().await;
        assert!(probe.probe);

        let next = breaker.admit();
        tokio::pin!(next);
        assert!(waiting(&mut next).await);
        drop(probe);
        assert!(next.await.probe, "the next waiter probes at once");
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_failure_does_not_extend_an_open_breaker() {
        let breaker = Arc::new(Breaker::default());
        let late = breaker.admit().await;
        trip(&breaker).await;
        assert_eq!(late.failed(), Transition::Unchanged);
        tokio::time::advance(RETRY).await;
        assert!(breaker.admit().await.probe, "the wait was not extended");
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_failure_while_probing_leaves_the_probe_to_judge() {
        let breaker = Arc::new(Breaker::default());
        let late = breaker.admit().await;
        trip(&breaker).await;
        tokio::time::advance(RETRY).await;
        let probe = breaker.admit().await;
        assert!(probe.probe);

        assert_eq!(late.failed(), Transition::Unchanged);
        let waiter = breaker.admit();
        tokio::pin!(waiter);
        assert!(waiting(&mut waiter).await, "the probe is still the one out");
        assert!(matches!(
            probe.failed(),
            Transition::Backoff { wait, .. } if wait == RETRY * 2
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_admitted_before_the_trip_loses_its_leave() {
        let breaker = Arc::new(Breaker::default());
        let early = breaker.admit().await;
        assert!(early.still_admitted());
        trip(&breaker).await;
        assert!(!early.still_admitted());

        tokio::time::advance(RETRY).await;
        let probe = breaker.admit().await;
        assert!(probe.still_admitted(), "the probe keeps its leave");
    }

    #[tokio::test(start_paused = true)]
    async fn breakers_are_independent_per_route_and_model() {
        let breakers = Breakers::default();
        let a = breakers.breaker("route", "model-a");
        trip(&a).await;

        assert!(Arc::ptr_eq(&a, &breakers.breaker("route", "model-a")));
        for other in [("route", "model-b"), ("other-route", "model-a")] {
            let breaker = breakers.breaker(other.0, other.1);
            assert!(!breaker.admit().await.probe);
            assert!(!breaker.is_open());
        }
    }
}
