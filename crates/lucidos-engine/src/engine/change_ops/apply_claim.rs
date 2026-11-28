use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use uuid::Uuid;

type LaneWaitRelease = Box<dyn Fn(Uuid) + Send + Sync>;

#[derive(Default)]
pub(crate) struct ApplyClaims {
    held: Mutex<HashMap<Uuid, Holder>>,
    next_generation: AtomicU64,
    lane_wait_release: OnceLock<LaneWaitRelease>,
}

/// INVARIANT: Who holds a change. The generation tells a holder from the one that took
/// the change over after it, so a late drop never releases the newer claim.
struct Holder {
    generation: u64,
    handed_off: Option<Arc<AtomicBool>>,
    lane_waiting: bool,
}

pub struct ApplyClaim {
    claims: Arc<ApplyClaims>,
    change_id: Uuid,
    generation: u64,
}

impl ApplyClaims {
    pub(crate) fn try_claim(self: &Arc<Self>, change_id: Uuid) -> Option<ApplyClaim> {
        self.claim(change_id, false)
    }

    /// INVARIANT: [`Self::try_claim`] for the landing lane. When another holder has the
    /// change, its release tells the lane, through the callback that
    /// [`Self::on_lane_wait_release`] set. That holder may never reach the
    /// lane: a refused Apply, Discard or Set aside lets go silently.
    pub(crate) fn try_claim_for_lane(self: &Arc<Self>, change_id: Uuid) -> Option<ApplyClaim> {
        self.claim(change_id, true)
    }

    pub(crate) fn is_held(&self, change_id: Uuid) -> bool {
        self.held().get(&change_id).is_some_and(|holder| {
            !holder
                .handed_off
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
        })
    }

    pub(crate) fn on_lane_wait_release(&self, release: impl Fn(Uuid) + Send + Sync + 'static) {
        if self.lane_wait_release.set(Box::new(release)).is_err() {
            log_legacy!("[ApplyClaim] the lane's release callback was already set");
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

    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Holder>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ApplyClaim {
    pub fn change_id(&self) -> Uuid {
        self.change_id
    }

    pub fn is_current(&self) -> bool {
        self.claims
            .held()
            .get(&self.change_id)
            .is_some_and(|h| h.generation == self.generation)
    }

    pub fn carried_by_session(&self, flag: Arc<AtomicBool>) {
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
        // INVARIANT: Outside the lock: the callback may claim again.
        if lane_waiting {
            if let Some(release) = self.claims.lane_wait_release.get() {
                release(self.change_id);
            }
        }
    }
}

impl crate::engine::LucidosEngine {
    pub async fn claim_pending_change(
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

pub const APPLY_IN_PROGRESS_MESSAGE: &str =
    "This change is already being applied. Its outcome arrives when that apply finishes.";

#[cfg(test)]
mod tests {
    use super::*;

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
