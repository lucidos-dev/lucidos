//! The *parent wake hold*: parent wakes the child-to-parent fan-in produces
//! while the engine is recovering at boot or tearing down.
//!
//! Recovery emits real child terminals. A coding-agent child a crash cut gets a
//! `CodingAgentIdled`, and the fan-in turns that into a `ChildThreadCompleted`
//! plus a wake. Run at once, the parent's turn races the recovery sweeps that
//! follow, and the chat orphan sweep reads the live turn as a dead one. So
//! `main.rs` engages the hold before recovery and releases it once recovery,
//! the boot refire and the event-wait rebuild are done.
//!
//! Teardown engages it too, and never releases it. A teardown's own aborts can
//! complete a child, and a turn opened then dies with the process. The card is
//! persisted, so the next boot's refire wakes the parent instead.
//! Why: `docs/plans/2026-10-03-crash-cut-child-reports-truthfully.md`.
//!
//! **Engaged only by boot and teardown.** A bus that never engages it sends
//! every wake straight to the channel, which is every test and every live path.

use std::sync::{Arc, Mutex};

use super::ParentCallback;

/// The wakes queued since [`ParentWakeHold::engage`], or `None` when the hold is
/// not engaged. `Clone` shares one register across every clone of the bus.
#[derive(Clone, Default)]
pub(crate) struct ParentWakeHold(Arc<Mutex<Option<Vec<ParentCallback>>>>);

impl ParentWakeHold {
    /// Start queueing wakes instead of sending them.
    pub(crate) fn engage(&self) {
        self.0.lock().unwrap().get_or_insert_with(Vec::new);
    }

    /// Queue `callback` while the hold is engaged, and hand it back otherwise.
    ///
    /// One completion card is one wake. The boot refire re-reads every card
    /// no parent reacted to, so it finds the cards recovery emitted this boot.
    /// A second wake for a queued card is therefore dropped here.
    pub(crate) fn intercept(&self, callback: ParentCallback) -> Option<ParentCallback> {
        let mut held = self.0.lock().unwrap();
        let Some(queue) = held.as_mut() else {
            return Some(callback);
        };
        if !queue
            .iter()
            .any(|q| q.child_completed_event_id == callback.child_completed_event_id)
        {
            queue.push(callback);
        }
        None
    }

    /// Stop holding, and hand back every queued wake in arrival order.
    pub(crate) fn release(&self) -> Vec<ParentCallback> {
        self.0.lock().unwrap().take().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn wake(card: Uuid) -> ParentCallback {
        ParentCallback {
            parent_thread_id: Uuid::new_v4(),
            child_thread_id: Uuid::new_v4(),
            child_completed_event_id: card,
            child_terminal_event_id: None,
            parent_is_coding_agent: false,
        }
    }

    #[test]
    fn a_hold_never_engaged_passes_every_wake_through() {
        let hold = ParentWakeHold::default();
        assert!(hold.intercept(wake(Uuid::new_v4())).is_some());
        assert!(hold.release().is_empty());
    }

    #[test]
    fn an_engaged_hold_queues_until_release_and_then_passes_through() {
        let hold = ParentWakeHold::default();
        hold.engage();
        let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(hold.intercept(wake(first)).is_none());
        assert!(hold.intercept(wake(second)).is_none());

        let released: Vec<Uuid> = hold
            .release()
            .into_iter()
            .map(|w| w.child_completed_event_id)
            .collect();
        assert_eq!(released, vec![first, second], "arrival order is kept");
        assert!(
            hold.intercept(wake(Uuid::new_v4())).is_some(),
            "after release the live path sends again"
        );
    }

    /// The boot refire finds the cards recovery just emitted, because their
    /// parents have not reacted yet. Both must come to one turn.
    #[test]
    fn a_second_wake_for_the_same_card_is_dropped() {
        let hold = ParentWakeHold::default();
        hold.engage();
        let card = Uuid::new_v4();
        assert!(hold.intercept(wake(card)).is_none());
        assert!(hold.intercept(wake(card)).is_none());
        assert_eq!(hold.release().len(), 1);
    }
}
