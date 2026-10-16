//! In-memory inbox for `POST /api/v1/presence-pong`. See
//! `system-knowhow/notifications.md` §3 for the protocol.

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
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

/// What the page already did with a notification when it answered. The page
/// acts first and reports after, so the push is suppressed only on its word
/// that the notification reached the reader in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceOutcome {
    /// The toast is on screen, alone or folded into the overflow toast.
    Shown,
    /// The source event is on screen, so the page marked the row read.
    AutoRead,
    /// Toasts are off, so the row waits on the bell badge.
    Silenced,
    /// The shell is not in front of the reader. The OS surface is theirs.
    NotShown,
}

impl PresenceOutcome {
    /// Strongest first: the order a shared connection merges its documents'
    /// answers in. The SDK reads it from the generated `engine-constants.ts`.
    pub const BY_STRENGTH: [Self; 4] =
        [Self::Shown, Self::AutoRead, Self::Silenced, Self::NotShown];

    pub fn suppresses_push(self) -> bool {
        !matches!(self, Self::NotShown)
    }
}

/// The page's response to a [`SystemEvent::PresenceCheck`].
#[derive(Debug, Clone, Deserialize)]
pub struct PresencePongRequest {
    pub notification_id: Uuid,
    pub device_id: String,
    pub outcome: PresenceOutcome,
}

/// Collected pong for a single notification, owned by the tracker until
/// the fan-out code decides.
#[derive(Debug, Clone)]
pub struct PresencePong {
    pub device_id: String,
    pub outcome: PresenceOutcome,
}

/// Spec §2 Step A: push_allowed iff no pong reports that the page already
/// handled the notification in the app.
pub fn decide_push_allowed(pongs: &[PresencePong]) -> bool {
    !pongs.iter().any(|p| p.outcome.suppresses_push())
}

struct PendingSlot {
    pongs: Vec<PresencePong>,
    notify: Arc<Notify>,
    expected: usize,
    /// `Some(push_allowed)` once [`PresenceTracker::decide`] ran. The slot
    /// stays until the sweep, so a late pong can tell what it missed.
    decided: Option<bool>,
    /// Wall-clock instant the slot was registered. Used to sweep slots,
    /// decided or not (a panicked fan-out task never decides), so the
    /// HashMap doesn't grow unbounded.
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
    /// time. The waiter wakes early when that many pongs arrive, or at the
    /// first one that suppresses the push, since that settles the decision.
    pub fn expect(&self, notification_id: Uuid, expected_pongs: usize) -> Arc<Notify> {
        let notify = Arc::new(Notify::new());
        let mut g = self.inner.lock().unwrap();

        // Sweep old slots, decided or not. Bounds the HashMap so a panicking
        // fan-out task that never decides doesn't leak forever.
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
                    decided: None,
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

    /// Record one pong. A pong after the decision, or for an unknown id, is
    /// discarded per spec §3 "Failure handling", and the HTTP handler always
    /// returns 200. A suppressing one that missed a push-allowed decision is
    /// logged: that page handled the notification, and the push went out too.
    pub fn record(&self, req: PresencePongRequest) {
        // Clone the Notify out of the slot so we drop the mutex before
        // signalling — keeps the lock release decoupled from any spurious
        // work `notify_one()` might do.
        let notify = {
            let mut g = self.inner.lock().unwrap();
            let Some(slot) = g.pending.get_mut(&req.notification_id) else {
                return;
            };
            if let Some(push_allowed) = slot.decided {
                if push_allowed && req.outcome.suppresses_push() {
                    crate::log!(
                        "[PresenceTracker] {:?} pong for {} from {} arrived after the push \
                         decision: the page handled it in the app AND the push went out",
                        req.outcome,
                        req.notification_id,
                        req.device_id
                    );
                }
                return;
            }
            let settles = req.outcome.suppresses_push();
            slot.pongs.push(PresencePong {
                device_id: req.device_id,
                outcome: req.outcome,
            });
            if settles || slot.pongs.len() >= slot.expected {
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

    /// Decide `push_allowed` for `notification_id` from the pongs recorded so
    /// far, and close the slot to further pongs in the same step. An unknown
    /// id has no pongs, so the push goes out.
    pub fn decide(&self, notification_id: Uuid) -> bool {
        let mut g = self.inner.lock().unwrap();
        let Some(slot) = g.pending.get_mut(&notification_id) else {
            return true;
        };
        *slot
            .decided
            .get_or_insert_with(|| decide_push_allowed(&std::mem::take(&mut slot.pongs)))
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

/// Route for the PresenceCheck pong inbox (system-knowhow/notifications.md §3).
pub(super) fn router() -> Router<AppState> {
    Router::new().route("/presence-pong", post(presence_pong))
}

#[cfg(test)]
mod tests {
    use super::*;

    use PresenceOutcome::{NotShown, Shown};

    fn req(
        notification_id: Uuid,
        device_id: &str,
        outcome: PresenceOutcome,
    ) -> PresencePongRequest {
        PresencePongRequest {
            notification_id,
            device_id: device_id.to_string(),
            outcome,
        }
    }

    #[test]
    fn s3_decide_reads_every_pong_recorded() {
        let tracker = PresenceTracker::new();
        let nid = Uuid::new_v4();
        let _notify = tracker.expect(nid, 2);

        tracker.record(req(nid, "dev-1", NotShown));
        tracker.record(req(nid, "dev-2", Shown));

        assert!(!tracker.decide(nid), "the second page showed the toast");
    }

    #[test]
    fn s3_decide_on_an_unknown_id_pushes() {
        let tracker = PresenceTracker::new();
        assert!(tracker.decide(Uuid::new_v4()));
    }

    #[test]
    fn s3_late_pong_after_decide_never_reopens_it() {
        // Spec §3 — pongs arriving after the decision ack 200 (race is normal).
        // Both directions: a late shown pong cannot take back a push that went
        // out, and a late not_shown one cannot undo a suppression.
        let tracker = PresenceTracker::new();
        let pushed = Uuid::new_v4();
        tracker.expect(pushed, 1);
        tracker.record(req(pushed, "dev-1", NotShown));
        assert!(tracker.decide(pushed));
        tracker.record(req(pushed, "dev-2", Shown));
        assert!(tracker.decide(pushed));

        let suppressed = Uuid::new_v4();
        tracker.expect(suppressed, 2);
        tracker.record(req(suppressed, "dev-1", Shown));
        assert!(!tracker.decide(suppressed));
        tracker.record(req(suppressed, "dev-2", NotShown));
        assert!(!tracker.decide(suppressed));
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
        tracker.record(req(nid, "dev-1", NotShown));
        tracker.record(req(nid, "dev-2", NotShown));

        waiter.await.unwrap();
        assert!(tracker.decide(nid));
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
        tracker.record(req(nid, "dev-1", NotShown));

        // Now wait — must resolve immediately, not block for the timeout.
        tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified())
            .await
            .expect("wakeup must survive the pre-await pong");
    }

    #[tokio::test]
    async fn s3_first_suppressing_pong_settles_without_waiting_for_the_rest() {
        let tracker = PresenceTracker::new();
        let nid = Uuid::new_v4();
        let notify = tracker.expect(nid, 3);

        tracker.record(req(nid, "dev-1", Shown));

        tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified())
            .await
            .expect("a shown pong settles the decision on its own");
    }

    #[tokio::test]
    async fn s3_a_not_shown_pong_alone_does_not_settle() {
        let tracker = PresenceTracker::new();
        let nid = Uuid::new_v4();
        let notify = tracker.expect(nid, 2);

        tracker.record(req(nid, "dev-1", NotShown));

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), notify.notified())
                .await
                .is_err(),
            "one not_shown pong of two expected must leave the engine waiting"
        );
    }

    #[test]
    fn s3_pong_body_is_the_outcome_the_page_reports() {
        let nid = Uuid::new_v4();
        let parsed: PresencePongRequest = serde_json::from_value(serde_json::json!({
            "notification_id": nid,
            "device_id": "dev-1",
            "outcome": "auto_read",
        }))
        .unwrap();
        assert_eq!(parsed.outcome, PresenceOutcome::AutoRead);
        assert!(parsed.outcome.suppresses_push());
    }

    #[test]
    fn s3_only_not_shown_leaves_the_push_to_the_os() {
        use PresenceOutcome::*;
        for outcome in [Shown, AutoRead, Silenced] {
            assert!(outcome.suppresses_push(), "{outcome:?}");
        }
        assert!(!NotShown.suppresses_push());
    }

    #[test]
    fn s3_expect_sweeps_stale_slots() {
        // The pending map must not grow unbounded if decide() is never
        // called (e.g. fan-out task panics after expect + emit).
        // A decided slot stays to answer late pongs, so it must age out too.
        let tracker = PresenceTracker::new();
        let stale_nid = Uuid::new_v4();
        let decided_nid = Uuid::new_v4();
        tracker.expect(stale_nid, 1);
        tracker.expect(decided_nid, 1);
        tracker.decide(decided_nid);

        // Manually age the slots past the sweep cutoff so we don't sleep
        // for STALE_SLOT_AFTER in a unit test.
        {
            let mut g = tracker.inner.lock().unwrap();
            for nid in [stale_nid, decided_nid] {
                g.pending.get_mut(&nid).unwrap().inserted_at =
                    Instant::now() - STALE_SLOT_AFTER - std::time::Duration::from_secs(1);
            }
        }

        // The next expect() sweeps stale entries.
        let fresh_nid = Uuid::new_v4();
        tracker.expect(fresh_nid, 1);

        let g = tracker.inner.lock().unwrap();
        assert!(
            !g.pending.contains_key(&stale_nid) && !g.pending.contains_key(&decided_nid),
            "stale slots must be swept on next expect()"
        );
        assert!(g.pending.contains_key(&fresh_nid));
    }
}
