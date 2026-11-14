//! One apply at a time per change.
//!
//! An apply claims its change before it reads the row, so the read and the act
//! sit inside one claim. A second apply is refused until the first lets go, and
//! one that claims after a finished apply reads `applied`. The claim lives in
//! memory: one engine owns the database (`startup_lease.rs`), and a restart
//! releases every claim with the process. ADR 0442.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use uuid::Uuid;

/// What a released claim tells the landing lane: the change it waited on.
type LaneWaitRelease = Box<dyn Fn(Uuid) + Send + Sync>;

/// The changes an apply holds right now.
#[derive(Default)]
pub(crate) struct ApplyClaims {
    held: Mutex<HashMap<Uuid, Holder>>,
    next_generation: AtomicU64,
    lane_wait_release: OnceLock<LaneWaitRelease>,
}

/// Who holds a change. The generation tells a holder from the one that took
/// the change over after it, so a late drop never releases the newer claim.
struct Holder {
    generation: u64,
    /// Set when the holder's merge session handed its duty to a continuation:
    /// the session's `external_continuation_requested` flag.
    handed_off: Option<Arc<AtomicBool>>,
    /// The landing lane found this change held and waits for its release.
    lane_waiting: bool,
}

/// Holds one change for one apply, including the merge session or recovery
/// task it hands off to. Dropping it lets the change go on every exit: an
/// error, a panic, a dropped request future.
pub(crate) struct ApplyClaim {
    claims: Arc<ApplyClaims>,
    change_id: Uuid,
    generation: u64,
}

impl ApplyClaims {
    /// Claim `change_id`, or `None` while another apply holds it. A holder
    /// whose merge session handed its duty to a continuation holds nothing:
    /// a wedged run can keep its claim long after it gave up the merge.
    pub(crate) fn try_claim(self: &Arc<Self>, change_id: Uuid) -> Option<ApplyClaim> {
        self.claim(change_id, false)
    }

    /// [`Self::try_claim`] for the landing lane. When another holder has the
    /// change, its release tells the lane, through the callback that
    /// [`Self::on_lane_wait_release`] set. That holder may never reach the
    /// lane: a refused Apply, Discard or Set aside lets go silently.
    pub(crate) fn try_claim_for_lane(self: &Arc<Self>, change_id: Uuid) -> Option<ApplyClaim> {
        self.claim(change_id, true)
    }

    /// Whether an apply holds `change_id` now. A holder whose merge session
    /// handed its duty off holds nothing, as in [`Self::try_claim`].
    pub(crate) fn is_held(&self, change_id: Uuid) -> bool {
        self.held().get(&change_id).is_some_and(|holder| {
            !holder
                .handed_off
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
        })
    }

    /// Set what a release the lane waits on calls, once, at lane start.
    pub(crate) fn on_lane_wait_release(&self, release: impl Fn(Uuid) + Send + Sync + 'static) {
        if self.lane_wait_release.set(Box::new(release)).is_err() {
            log!("[ApplyClaim] the lane's release callback was already set");
        }
    }

    fn claim(self: &Arc<Self>, change_id: Uuid, for_lane: bool) -> Option<ApplyClaim> {
        let mut held = self.held();
        let mut lane_waiting = false;
        if let Some(holder) = held.get_mut(&change_id) {
            let handed_off = holder
                .handed_off
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire));
            if !handed_off {
                holder.lane_waiting |= for_lane;
                return None;
            }
            // A continuation taking over inherits what the lane waits on.
            lane_waiting = holder.lane_waiting && !for_lane;
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        held.insert(
            change_id,
            Holder {
                generation,
                handed_off: None,
                lane_waiting,
            },
        );
        Some(ApplyClaim {
            claims: self.clone(),
            change_id,
            generation,
        })
    }

    /// A panic under this lock leaves the map consistent.
    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Holder>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ApplyClaim {
    pub(crate) fn change_id(&self) -> Uuid {
        self.change_id
    }

    /// Whether this claim still holds its change, or a continuation took it.
    pub(crate) fn is_current(&self) -> bool {
        self.claims
            .held()
            .get(&self.change_id)
            .is_some_and(|h| h.generation == self.generation)
    }

    /// Tie the claim to the merge session that carries it. Once that session
    /// hands its duty off (`flag` set), the continuation may take the claim.
    pub(crate) fn carried_by_session(&self, flag: Arc<AtomicBool>) {
        if let Some(holder) = self
            .claims
            .held()
            .get_mut(&self.change_id)
            .filter(|h| h.generation == self.generation)
        {
            holder.handed_off = Some(flag);
        }
    }
}

impl Drop for ApplyClaim {
    fn drop(&mut self) {
        let lane_waiting = {
            let mut held = self.claims.held();
            if held
                .get(&self.change_id)
                .is_some_and(|h| h.generation == self.generation)
            {
                held.remove(&self.change_id).is_some_and(|h| h.lane_waiting)
            } else {
                false
            }
        };
        // Outside the lock: the callback may claim again.
        if lane_waiting {
            if let Some(release) = self.claims.lane_wait_release.get() {
                release(self.change_id);
            }
        }
    }
}

impl crate::engine::LucidosEngine {
    /// Claim a change found by an earlier read, then read it again under the
    /// claim. `None` when another apply holds it or it is no longer pending:
    /// that apply, or whatever resolved the change, announces the outcome.
    pub(crate) async fn claim_pending_change(
        &self,
        change_id: Uuid,
    ) -> Result<Option<ApplyClaim>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(claim) = self.apply_claims.try_claim(change_id) else {
            return Ok(None);
        };
        Ok(self
            .changes()
            .get_by_id(change_id)
            .await?
            .filter(|change| change.is_pending())
            .map(|_| claim))
    }
}

/// What an apply refused by a held claim tells its caller.
pub(crate) const APPLY_IN_PROGRESS_MESSAGE: &str =
    "This change is already being applied. Its outcome arrives when that apply finishes.";

#[cfg(test)]
mod tests {
    use super::*;

    /// Two applies of one change. Apply A claims, then parks where the real
    /// apply runs its tiers. Apply B arrives in that gap. Before the claim,
    /// both read the row as pending and both reset the shared temp worktree.
    #[tokio::test]
    async fn a_running_apply_refuses_a_second_apply_of_the_same_change() {
        let claims = Arc::new(ApplyClaims::default());
        let change = Uuid::new_v4();
        let a_claimed = Arc::new(tokio::sync::Barrier::new(2));
        let b_done = Arc::new(tokio::sync::Barrier::new(2));

        let apply_a = tokio::spawn({
            let (claims, a_claimed, b_done) = (claims.clone(), a_claimed.clone(), b_done.clone());
            async move {
                let claim = claims.try_claim(change);
                a_claimed.wait().await;
                b_done.wait().await;
                claim.is_some()
            }
        });
        let apply_b = tokio::spawn({
            let claims = claims.clone();
            async move {
                a_claimed.wait().await;
                let claimed = claims.try_claim(change).is_some();
                b_done.wait().await;
                claimed
            }
        });

        assert!(apply_a.await.unwrap());
        assert!(!apply_b.await.unwrap(), "the second apply waits its turn");
        assert!(
            claims.try_claim(change).is_some(),
            "the change is free once the first apply lets go"
        );
    }

    /// A held change reads held until the claim drops, or until its session
    /// hands the duty off.
    #[test]
    fn is_held_follows_the_claim_and_its_handoff() {
        let claims = Arc::new(ApplyClaims::default());
        let change = Uuid::new_v4();
        assert!(!claims.is_held(change));
        let claim = claims.try_claim(change).expect("free change");
        assert!(claims.is_held(change));
        let flag = Arc::new(AtomicBool::new(false));
        claim.carried_by_session(flag.clone());
        assert!(claims.is_held(change));
        flag.store(true, Ordering::Release);
        assert!(!claims.is_held(change));
        flag.store(false, Ordering::Release);
        drop(claim);
        assert!(!claims.is_held(change));
    }

    /// A merge session wedged past the watchdog keeps its claim until its run
    /// returns, which may be never. Once it hands the duty to a continuation,
    /// the continuation takes the claim, and the old drop leaves it alone.
    #[test]
    fn a_continuation_takes_over_the_claim_of_a_run_that_handed_off() {
        let claims = Arc::new(ApplyClaims::default());
        let change = Uuid::new_v4();
        let wedged = claims.try_claim(change).unwrap();
        let handed_off = Arc::new(AtomicBool::new(false));
        wedged.carried_by_session(handed_off.clone());
        assert!(
            claims.try_claim(change).is_none(),
            "still carrying the duty"
        );

        handed_off.store(true, Ordering::Release);
        let continuation = claims.try_claim(change).expect("the duty moved on");
        drop(wedged);
        assert!(
            claims.try_claim(change).is_none(),
            "the wedged run's drop does not release the continuation's claim"
        );
        drop(continuation);
        assert!(claims.try_claim(change).is_some());
    }

    /// The lane found a change held, and the holder lets go without reaching
    /// the lane, as a refused Apply does. Its release tells the lane, once. A
    /// release the lane never waited on, and the lane's own, tell it nothing.
    #[test]
    fn a_release_the_lane_waited_on_tells_the_lane() {
        let claims = Arc::new(ApplyClaims::default());
        let told = Arc::new(Mutex::new(Vec::new()));
        claims.on_lane_wait_release({
            let told = told.clone();
            move |change| told.lock().unwrap().push(change)
        });
        let change = Uuid::new_v4();

        drop(claims.try_claim(change).unwrap());
        assert!(told.lock().unwrap().is_empty(), "the lane never waited");

        let refused_apply = claims.try_claim(change).unwrap();
        assert!(claims.try_claim_for_lane(change).is_none());
        drop(refused_apply);
        assert_eq!(*told.lock().unwrap(), vec![change]);

        let lane = claims.try_claim_for_lane(change).expect("free again");
        assert!(claims.try_claim(change).is_none());
        drop(lane);
        assert_eq!(
            *told.lock().unwrap(),
            vec![change],
            "the lane's own goes quietly"
        );
    }

    /// A continuation takes over a claim the lane waits on. The old holder's
    /// drop tells the lane nothing, and the continuation's release does.
    #[test]
    fn a_continuation_owes_the_lane_the_release() {
        let claims = Arc::new(ApplyClaims::default());
        let told = Arc::new(Mutex::new(Vec::new()));
        claims.on_lane_wait_release({
            let told = told.clone();
            move |change| told.lock().unwrap().push(change)
        });
        let change = Uuid::new_v4();
        let wedged = claims.try_claim(change).unwrap();
        let handed_off = Arc::new(AtomicBool::new(false));
        wedged.carried_by_session(handed_off.clone());
        assert!(claims.try_claim_for_lane(change).is_none());

        handed_off.store(true, Ordering::Release);
        let continuation = claims.try_claim(change).expect("the duty moved on");
        drop(wedged);
        assert!(told.lock().unwrap().is_empty());
        drop(continuation);
        assert_eq!(*told.lock().unwrap(), vec![change]);
    }

    #[test]
    fn claims_are_per_change() {
        let claims = Arc::new(ApplyClaims::default());
        let _a = claims.try_claim(Uuid::new_v4()).unwrap();
        assert!(claims.try_claim(Uuid::new_v4()).is_some());
    }

    /// ADR 0060 rejected an in-memory claim because a missed clear wedges
    /// Apply. A drop guard clears on every exit, a panic included.
    #[tokio::test]
    async fn a_claim_never_outlives_its_holder() {
        let claims = Arc::new(ApplyClaims::default());
        let change = Uuid::new_v4();

        fn failing_apply(claims: &Arc<ApplyClaims>, change: Uuid) -> Result<(), &'static str> {
            let _claim = claims.try_claim(change).unwrap();
            Err("the apply failed")
        }
        let errored = failing_apply(&claims, change);
        assert!(errored.is_err());
        assert!(claims.try_claim(change).is_some(), "an error lets go");

        let panicked = tokio::spawn({
            let claims = claims.clone();
            async move {
                let _claim = claims.try_claim(change).unwrap();
                panic!("the apply panicked");
            }
        });
        assert!(panicked.await.is_err());
        assert!(claims.try_claim(change).is_some(), "a panic lets go");

        let (claimed_tx, claimed_rx) = tokio::sync::oneshot::channel();
        let parked = tokio::spawn({
            let claims = claims.clone();
            async move {
                let _claim = claims.try_claim(change).unwrap();
                claimed_tx.send(()).unwrap();
                std::future::pending::<()>().await;
            }
        });
        claimed_rx.await.unwrap();
        assert!(
            claims.try_claim(change).is_none(),
            "the parked apply holds it"
        );
        parked.abort();
        assert!(parked.await.unwrap_err().is_cancelled());
        assert!(
            claims.try_claim(change).is_some(),
            "a dropped request future lets go"
        );
    }
}
