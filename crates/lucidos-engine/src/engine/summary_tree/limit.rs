//! The compactor's adaptive concurrency: one *lane* per provider route.
//!
//! A lane holds at most `limit` model calls at once, and the limit follows
//! AIMD, as TCP congestion control does. Each success adds `1 / limit`, so a
//! full window of successes adds one call. A capacity failure halves it, once
//! per window: a call that started before the last halving cannot halve it
//! again, so one burst of rate limits counts once.
//!
//! An urgent call, for a live event, takes a freed permit before any backfill
//! call does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

/// Calls a new lane allows at once, before it has learned anything.
pub(crate) const START_LIMIT: usize = 64;

/// Calls a lane allows at once at most.
pub(crate) const MAX_LIMIT: usize = 128;

const MIN_LIMIT: f64 = 1.0;

/// Every lane the compactor has called through, by route.
#[derive(Default)]
pub(crate) struct Lanes {
    lanes: Mutex<HashMap<String, Arc<Lane>>>,
}

impl Lanes {
    pub(crate) fn lane(&self, route: &str) -> Arc<Lane> {
        self.lanes
            .lock()
            .unwrap()
            .entry(route.to_string())
            .or_insert_with(|| Arc::new(Lane::new(START_LIMIT)))
            .clone()
    }
}

pub(crate) struct Lane {
    state: Mutex<State>,
    freed: Notify,
}

struct State {
    limit: f64,
    in_flight: usize,
    /// Bumped by each halving. A permit from an older window cannot halve.
    window: u64,
    urgent_waiting: usize,
}

impl State {
    fn cap(&self) -> usize {
        self.limit.floor() as usize
    }

    fn has_room(&self, urgent: bool) -> bool {
        self.in_flight < self.cap() && (urgent || self.urgent_waiting == 0)
    }
}

impl Lane {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(State {
                limit: (limit as f64).clamp(MIN_LIMIT, MAX_LIMIT as f64),
                in_flight: 0,
                window: 0,
                urgent_waiting: 0,
            }),
            freed: Notify::new(),
        }
    }

    /// Wait for room, then hold one call's place until the permit drops.
    pub(crate) async fn acquire(self: &Arc<Self>, urgent: bool) -> Permit {
        let mut queued = urgent.then(|| UrgentWaiter::new(self));
        loop {
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if state.has_room(urgent) {
                    state.in_flight += 1;
                    if let Some(waiter) = queued.take() {
                        waiter.served(&mut state);
                        // A backfill call that saw this one waiting may fit now.
                        if state.has_room(false) {
                            self.freed.notify_waiters();
                        }
                    }
                    return Permit {
                        lane: self.clone(),
                        window: state.window,
                    };
                }
            }
            freed.await;
        }
    }

    /// The calls this lane allows at once now.
    #[cfg(test)]
    pub(crate) fn limit(&self) -> usize {
        self.state.lock().unwrap().cap()
    }

    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.state.lock().unwrap().in_flight
    }
}

/// Counts an urgent caller as waiting until it is served or gives up, so no
/// backfill call takes the permit it waits for.
struct UrgentWaiter {
    lane: Arc<Lane>,
    served: bool,
}

impl UrgentWaiter {
    fn new(lane: &Arc<Lane>) -> Self {
        lane.state.lock().unwrap().urgent_waiting += 1;
        Self {
            lane: lane.clone(),
            served: false,
        }
    }

    /// Called under the lane's lock, so the drop must not take it again.
    fn served(mut self, state: &mut State) {
        state.urgent_waiting -= 1;
        self.served = true;
    }
}

impl Drop for UrgentWaiter {
    fn drop(&mut self) {
        if !self.served {
            self.lane.state.lock().unwrap().urgent_waiting -= 1;
            self.lane.freed.notify_waiters();
        }
    }
}

/// One call's place in a lane. Report how the call went, or drop it to say
/// nothing about capacity.
pub(crate) struct Permit {
    lane: Arc<Lane>,
    window: u64,
}

impl Permit {
    /// The provider answered: grow the limit by one per full window.
    pub(crate) fn succeeded(self) {
        let mut state = self.lane.state.lock().unwrap();
        state.limit = (state.limit + 1.0 / state.limit).min(MAX_LIMIT as f64);
    }

    /// The provider is out of capacity for us: halve the limit, unless a call
    /// of this window already did.
    pub(crate) fn congested(self) {
        let mut state = self.lane.state.lock().unwrap();
        if state.window == self.window {
            state.limit = (state.limit / 2.0).max(MIN_LIMIT);
            state.window += 1;
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.lane.state.lock().unwrap().in_flight -= 1;
        self.lane.freed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Whether `acquire` is still waiting after the runtime had a chance to
    /// run it.
    async fn still_waiting<F: std::future::Future>(f: &mut std::pin::Pin<&mut F>) -> bool {
        tokio::time::timeout(Duration::from_millis(20), f.as_mut())
            .await
            .is_err()
    }

    #[tokio::test]
    async fn a_lane_never_holds_more_calls_than_its_limit() {
        let lane = Arc::new(Lane::new(3));
        let held: Vec<Permit> =
            futures::future::join_all((0..3).map(|_| lane.acquire(false))).await;
        assert_eq!(lane.in_flight(), 3);

        let fourth = lane.acquire(false);
        tokio::pin!(fourth);
        assert!(still_waiting(&mut fourth).await, "a fourth call waits");

        drop(held);
        let _fourth = fourth.await;
        assert_eq!(lane.in_flight(), 1);
    }

    #[tokio::test]
    async fn one_burst_of_failures_halves_the_limit_once() {
        let lane = Arc::new(Lane::new(64));
        let burst: Vec<Permit> =
            futures::future::join_all((0..10).map(|_| lane.acquire(false))).await;
        for permit in burst {
            permit.congested();
        }
        assert_eq!(lane.limit(), 32);

        lane.acquire(false).await.congested();
        assert_eq!(lane.limit(), 16, "a call of the next window halves again");
    }

    #[tokio::test]
    async fn the_limit_never_falls_below_one() {
        let lane = Arc::new(Lane::new(2));
        for _ in 0..5 {
            lane.acquire(false).await.congested();
        }
        assert_eq!(lane.limit(), 1);
        let _one = lane.acquire(false).await;
    }

    #[tokio::test]
    async fn success_grows_the_limit_by_one_per_window_up_to_the_cap() {
        // Each success adds 1 / limit as the limit grows, so a window of 8
        // reaches just under 9 and the next success crosses it.
        let lane = Arc::new(Lane::new(8));
        for _ in 0..8 {
            lane.acquire(false).await.succeeded();
        }
        assert_eq!(lane.limit(), 8);
        lane.acquire(false).await.succeeded();
        assert_eq!(lane.limit(), 9);

        let near_cap = Arc::new(Lane::new(MAX_LIMIT));
        for _ in 0..1_000 {
            near_cap.acquire(false).await.succeeded();
        }
        assert_eq!(near_cap.limit(), MAX_LIMIT);
    }

    #[tokio::test]
    async fn a_dropped_permit_says_nothing_about_capacity() {
        let lane = Arc::new(Lane::new(8));
        drop(lane.acquire(false).await);
        assert_eq!(lane.limit(), 8);
    }

    #[tokio::test]
    async fn an_urgent_call_takes_the_freed_permit_first() {
        let lane = Arc::new(Lane::new(1));
        let held = lane.acquire(false).await;

        let background = lane.acquire(false);
        tokio::pin!(background);
        assert!(still_waiting(&mut background).await);
        let urgent = lane.acquire(true);
        tokio::pin!(urgent);
        assert!(still_waiting(&mut urgent).await);

        drop(held);
        let urgent_permit = urgent.await;
        assert!(
            still_waiting(&mut background).await,
            "the backfill call still waits"
        );

        drop(urgent_permit);
        let _background = background.await;
    }

    #[tokio::test]
    async fn an_urgent_caller_that_gives_up_frees_the_backfill() {
        let lane = Arc::new(Lane::new(1));
        let held = lane.acquire(false).await;
        {
            let urgent = lane.acquire(true);
            tokio::pin!(urgent);
            assert!(still_waiting(&mut urgent).await);
        }
        drop(held);
        let _background = lane.acquire(false).await;
    }

    #[test]
    fn each_route_has_its_own_lane() {
        let lanes = Lanes::default();
        let a = lanes.lane("openai/gpt-6.1-sol");
        assert!(Arc::ptr_eq(&a, &lanes.lane("openai/gpt-6.1-sol")));
        assert!(!Arc::ptr_eq(&a, &lanes.lane("vertex/gemini-3.8-flash")));
        assert_eq!(a.limit(), START_LIMIT);
    }
}
