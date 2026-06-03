//! In-memory inbox for `POST /api/v1/presence-pong`. See
//! `system-knowhow/notifications.md` §3 for the protocol.

use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::api::AppState;

/// Slots older than this are swept on the next `expect()` call. The
/// PresenceCheck deadline (`scheduler::push::DEADLINE_MS`) sits well
/// under this; 5 s leaves plenty of headroom for a slow tokio scheduler
/// tick while bounding leak size to "almost nothing" in steady state.
const STALE_SLOT_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

/// The page's response to a [`SystemEvent::PresenceCheck`].
#[derive(Debug, Clone, Deserialize)]
pub struct PresencePongRequest {
    pub notification_id: Uuid,
    pub device_id: String,
    pub is_active: bool,
    #[serde(default)]
    pub focused_thread_id: Option<Uuid>,
    #[serde(default)]
    pub event_in_viewport: bool,
}

/// Collected pong for a single notification, owned by the tracker until
/// drained by the fan-out code.
#[derive(Debug, Clone)]
pub struct PresencePong {
    pub device_id: String,
    pub is_active: bool,
    pub focused_thread_id: Option<Uuid>,
    pub event_in_viewport: bool,
}

struct PendingSlot {
    pongs: Vec<PresencePong>,
    notify: Arc<Notify>,
    expected: usize,
    /// Wall-clock instant the slot was registered. Used to sweep slots
    /// whose `collect()` never ran (panicked fan-out task, future
    /// caller that forgets) so the HashMap doesn't grow unbounded.
    inserted_at: Instant,
}

#[derive(Default)]
struct TrackerInner {
    pending: HashMap<Uuid, PendingSlot>,
}

#[derive(Default, Clone)]
pub struct PresenceTracker {
    inner: Arc<Mutex<TrackerInner>>,
}

impl PresenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register interest in pongs for `notification_id`. `expected_pongs`
    /// is the number of candidate devices the engine resolved at fan-out
    /// time — when that many pongs arrive the waiter is woken early so
    /// the engine doesn't burn the full `deadline_ms`.
    pub fn expect(&self, notification_id: Uuid, expected_pongs: usize) -> Arc<Notify> {
        let notify = Arc::new(Notify::new());
        let mut g = self.inner.lock().unwrap();

        // Sweep slots that never got collected. Bounds the HashMap so a
        // panicking fan-out task or a future caller forgetting collect()
        // doesn't leak forever.
        let cutoff = Instant::now() - STALE_SLOT_AFTER;
        g.pending.retain(|_, slot| slot.inserted_at > cutoff);

        let existed = g
            .pending
            .insert(
                notification_id,
                PendingSlot {
                    pongs: Vec::new(),
                    notify: notify.clone(),
                    expected: expected_pongs,
                    inserted_at: Instant::now(),
                },
            )
            .is_some();
        if existed {
            // UUIDs are collision-free in practice; if this fires the
            // fan-out is being invoked twice for the same id (upstream bug).
            crate::log!(
                "[PresenceTracker] expect() overwrote an existing slot for {} \
                 — dropped any pongs collected so far",
                notification_id
            );
        }
        notify
    }

    /// Record one pong. Late pongs (no slot) are silently discarded per
    /// spec §3 "Failure handling"; the HTTP handler always returns 200.
    pub fn record(&self, req: PresencePongRequest) {
        // Clone the Notify out of the slot so we drop the mutex before
        // signalling — keeps the lock release decoupled from any spurious
        // work `notify_one()` might do.
        let notify = {
            let mut g = self.inner.lock().unwrap();
            let Some(slot) = g.pending.get_mut(&req.notification_id) else {
                return;
            };
            slot.pongs.push(PresencePong {
                device_id: req.device_id,
                is_active: req.is_active,
                focused_thread_id: req.focused_thread_id,
                event_in_viewport: req.event_in_viewport,
            });
            if slot.pongs.len() >= slot.expected {
                Some(slot.notify.clone())
            } else {
                None
            }
        };
        // notify_one stores a permit, so a wakeup arriving before the
        // waiter calls .notified() is preserved — using notify_waiters()
        // here would lose the wakeup and burn the full deadline.
        if let Some(n) = notify {
            n.notify_one();
        }
    }

    /// Drain the pong vec for `notification_id` and forget the slot.
    pub fn collect(&self, notification_id: Uuid) -> Vec<PresencePong> {
        self.inner
            .lock()
            .unwrap()
            .pending
            .remove(&notification_id)
            .map(|s| s.pongs)
            .unwrap_or_default()
    }
}

pub async fn presence_pong(
    State(state): State<AppState>,
    Json(req): Json<PresencePongRequest>,
) -> StatusCode {
    state.engine.presence_tracker.record(req);
    // Always 200 — late pongs are normal, no value in surfacing the race
    // back to the page. See spec §3 "Failure handling".
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(notification_id: Uuid, device_id: &str, is_active: bool) -> PresencePongRequest {
        PresencePongRequest {
            notification_id,
            device_id: device_id.to_string(),
            is_active,
            focused_thread_id: None,
            event_in_viewport: false,
        }
    }

    #[test]
    fn s3_presence_tracker_collects_pongs_in_order() {
        let tracker = PresenceTracker::new();
        let nid = Uuid::new_v4();
        let _notify = tracker.expect(nid, 2);

        tracker.record(req(nid, "dev-1", true));
        tracker.record(req(nid, "dev-2", false));

        let pongs = tracker.collect(nid);
        assert_eq!(pongs.len(), 2);
        assert_eq!(pongs[0].device_id, "dev-1");
        assert!(pongs[0].is_active);
        assert_eq!(pongs[1].device_id, "dev-2");
        assert!(!pongs[1].is_active);
    }

    #[test]
    fn s3_collect_without_expect_returns_empty() {
        // Stray collect on an unknown id — defensive, used only by tests today.
        let tracker = PresenceTracker::new();
        assert!(tracker.collect(Uuid::new_v4()).is_empty());
    }

    #[test]
    fn s3_late_pong_after_collect_is_silently_dropped() {
        // Spec §3 — pongs arriving after the deadline ack 200 (race is normal).
        let tracker = PresenceTracker::new();
        let nid = Uuid::new_v4();
        tracker.expect(nid, 1);
        tracker.record(req(nid, "dev-1", true));
        let _ = tracker.collect(nid);
        // Slot is gone — late pong from a slower second tab.
        tracker.record(req(nid, "dev-2", false));
        // No second collect; just verify we didn't panic.
        assert!(tracker.collect(nid).is_empty());
    }

    #[tokio::test]
    async fn s3_all_candidates_pong_short_circuits_deadline() {
        // When every expected device pongs, the waiter wakes immediately
        // instead of waiting out the deadline.
        let tracker = PresenceTracker::new();
        let nid = Uuid::new_v4();
        let notify = tracker.expect(nid, 2);

        let waiter = tokio::spawn({
            let notify = notify.clone();
            async move {
                tokio::time::timeout(std::time::Duration::from_secs(5), notify.notified())
                    .await
                    .expect("notify must fire before 5s")
            }
        });

        // Give the waiter a chance to register.
        tokio::task::yield_now().await;
        tracker.record(req(nid, "dev-1", true));
        tracker.record(req(nid, "dev-2", false));

        waiter.await.unwrap();
        let pongs = tracker.collect(nid);
        assert_eq!(pongs.len(), 2);
    }

    #[tokio::test]
    async fn s3_pong_arriving_before_waiter_does_not_lose_wakeup() {
        // Regression: an earlier impl used notify_waiters(), which only
        // wakes CURRENT waiters. In the realistic flow (expect → emit
        // SSE → SDK pong round-trip → record → tokio task picks up
        // .notified()), pongs can land before the fan-out code calls
        // .notified(). notify_one() stores a permit so the wakeup
        // survives the gap.
        let tracker = PresenceTracker::new();
        let nid = Uuid::new_v4();
        let notify = tracker.expect(nid, 1);

        // Record BEFORE any waiter exists.
        tracker.record(req(nid, "dev-1", true));

        // Now wait — must resolve immediately, not block for the timeout.
        tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified())
            .await
            .expect("wakeup must survive the pre-await pong");
    }

    #[test]
    fn s3_expect_sweeps_stale_slots() {
        // The pending map must not grow unbounded if collect() is never
        // called (e.g. fan-out task panics after expect + emit).
        let tracker = PresenceTracker::new();
        let stale_nid = Uuid::new_v4();
        tracker.expect(stale_nid, 1);

        // Manually age the slot past the sweep cutoff so we don't sleep
        // for STALE_SLOT_AFTER in a unit test.
        {
            let mut g = tracker.inner.lock().unwrap();
            let slot = g.pending.get_mut(&stale_nid).unwrap();
            slot.inserted_at = Instant::now() - STALE_SLOT_AFTER - std::time::Duration::from_secs(1);
        }

        // The next expect() sweeps stale entries.
        let fresh_nid = Uuid::new_v4();
        tracker.expect(fresh_nid, 1);

        let g = tracker.inner.lock().unwrap();
        assert!(
            !g.pending.contains_key(&stale_nid),
            "stale slot must be swept on next expect()"
        );
        assert!(g.pending.contains_key(&fresh_nid));
    }
}
