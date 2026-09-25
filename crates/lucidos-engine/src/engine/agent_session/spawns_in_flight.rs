//! Coding-agent spawns that reached the spawn debounce but have not returned.
//!
//! `agent_sessions` learns about a spawn only once its subprocess registers,
//! which can take many seconds: worktree setup, git calls, and the two-permit
//! `cc_startup_semaphore` all come first. A caller asking "does anybody own
//! this thread?" must count that window too, or it settles a turn that is
//! about to start.

use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

/// Per-thread count of spawns between the spawn debounce and their return.
#[derive(Default)]
pub(crate) struct SpawnsInFlight(Mutex<HashMap<Uuid, usize>>);

/// Holds one thread's in-flight slot. Dropping it releases the slot, so every
/// return path of `run_direct_agent` releases it, early errors included.
pub(crate) struct SpawnInFlight<'a> {
    spawns: &'a SpawnsInFlight,
    thread_id: Uuid,
}

impl SpawnsInFlight {
    pub(crate) fn enter(&self, thread_id: Uuid) -> SpawnInFlight<'_> {
        *self.lock().entry(thread_id).or_default() += 1;
        SpawnInFlight {
            spawns: self,
            thread_id,
        }
    }

    pub(crate) fn contains(&self, thread_id: Uuid) -> bool {
        self.lock().contains_key(&thread_id)
    }

    /// A panic while holding this lock leaves the map consistent, so a
    /// poisoned lock is safe to keep using.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, usize>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for SpawnInFlight<'_> {
    fn drop(&mut self) {
        let mut map = self.spawns.lock();
        if let Some(count) = map.get_mut(&self.thread_id) {
            *count -= 1;
            if *count == 0 {
                map.remove(&self.thread_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawn_owns_its_thread_until_it_returns() {
        let spawns = SpawnsInFlight::default();
        let thread = Uuid::new_v4();
        let other = Uuid::new_v4();
        {
            let _slot = spawns.enter(thread);
            assert!(spawns.contains(thread));
            assert!(!spawns.contains(other), "the slot is per thread");
        }
        assert!(!spawns.contains(thread), "dropping the slot releases it");
    }

    #[test]
    fn overlapping_spawns_release_the_thread_only_when_the_last_one_returns() {
        let spawns = SpawnsInFlight::default();
        let thread = Uuid::new_v4();
        let first = spawns.enter(thread);
        let second = spawns.enter(thread);
        drop(first);
        assert!(
            spawns.contains(thread),
            "the second spawn still owns the thread"
        );
        drop(second);
        assert!(!spawns.contains(thread));
    }
}
