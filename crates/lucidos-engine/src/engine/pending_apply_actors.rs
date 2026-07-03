//! Per-`change_id` stash for the user actor of an in-flight Apply.
//!
//! When the user clicks Apply on a pending change whose Claude Code session has exited
//! (Tier 3 slow path with conflict resolution), `apply_change` has the user's
//! actor in scope but spawns a *new* Claude Code subprocess for the merge. By the time
//! that subprocess finishes and `agent_session::run_session` cleanup emits
//! `ChangeApplied` / `ChangeApplyFailed`, the original HTTP call has long
//! returned and the actor is gone — the cleanup used to stamp `actor: None`,
//! which collapses to "Lucidos Engine" in the chat chip.
//!
//! This stash lets the apply call site park the actor by `change_id` before
//! spawning the merge session; the cleanup code then takes it back out and
//! stamps it on the resulting event. Take is one-shot (the `HashMap::remove`
//! contract): if no cleanup ever runs (e.g. Claude Code subprocess fails to start) the
//! entry leaks until the engine restarts — a single `MessageOrigin` per failed
//! apply, not worth refcounting around.

use crate::engine::thread_events::MessageOrigin;
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct PendingApplyActors {
    inner: Mutex<HashMap<Uuid, MessageOrigin>>,
}

impl PendingApplyActors {
    /// Park `actor` for `change_id`. Replaces any existing entry — the latest
    /// Apply click wins, which matches the user-visible "second click wins"
    /// behavior of the apply pipeline itself.
    pub(crate) fn stash(&self, change_id: Uuid, actor: MessageOrigin) {
        match self.inner.lock() {
            Ok(mut g) => {
                g.insert(change_id, actor);
            }
            Err(e) => log!(
                "[PendingApplyActors] mutex poisoned on stash for change {}: {} — actor dropped, ChangeApplied will fall back to engine attribution",
                change_id,
                e,
            ),
        }
    }

    /// Remove and return the actor parked for `change_id`, if any. One-shot:
    /// the second call returns `None` so a cleanup that fires twice (e.g. on a
    /// retry) doesn't double-stamp.
    pub(crate) fn take(&self, change_id: Uuid) -> Option<MessageOrigin> {
        match self.inner.lock() {
            Ok(mut g) => g.remove(&change_id),
            Err(e) => {
                log!(
                    "[PendingApplyActors] mutex poisoned on take for change {}: {} — falling back to engine attribution",
                    change_id,
                    e,
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, label: &str) -> MessageOrigin {
        MessageOrigin::Device {
            device_id: id.into(),
            label: label.into(),
        }
    }

    #[test]
    fn stash_then_take_round_trips_the_actor() {
        let store = PendingApplyActors::default();
        let id = Uuid::new_v4();
        let actor = device("iphone-1", "iOS Safari PWA");

        store.stash(id, actor.clone());

        assert_eq!(store.take(id), Some(actor));
    }

    #[test]
    fn take_is_one_shot() {
        let store = PendingApplyActors::default();
        let id = Uuid::new_v4();
        store.stash(id, device("d", "l"));

        assert!(store.take(id).is_some());
        assert!(
            store.take(id).is_none(),
            "second take must return None — otherwise a retried cleanup would double-stamp"
        );
    }

    #[test]
    fn take_for_unstashed_change_id_returns_none() {
        let store = PendingApplyActors::default();
        assert!(store.take(Uuid::new_v4()).is_none());
    }

    #[test]
    fn restash_replaces_previous_actor() {
        // The user clicks Apply, the call stashes actor A. Before the merge
        // completes, they click Apply again on the same change (e.g. from a
        // different device) — the new call stashes actor B. The cleanup that
        // eventually fires must see B (the active intent), not A.
        let store = PendingApplyActors::default();
        let id = Uuid::new_v4();
        store.stash(id, device("phone", "Phone"));
        store.stash(id, device("laptop", "Laptop"));

        match store.take(id) {
            Some(MessageOrigin::Device { device_id, .. }) => assert_eq!(device_id, "laptop"),
            other => panic!("expected latest stash to win, got {:?}", other),
        }
    }

    #[test]
    fn entries_are_independent_per_change_id() {
        let store = PendingApplyActors::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        store.stash(a, device("a-id", "A"));
        store.stash(b, device("b-id", "B"));

        match store.take(a) {
            Some(MessageOrigin::Device { device_id, .. }) => assert_eq!(device_id, "a-id"),
            other => panic!("expected A, got {:?}", other),
        }
        match store.take(b) {
            Some(MessageOrigin::Device { device_id, .. }) => assert_eq!(device_id, "b-id"),
            other => panic!("expected B, got {:?}", other),
        }
    }
}
