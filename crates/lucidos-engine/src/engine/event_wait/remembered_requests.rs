//! The last request of each Lucidos Agent thread, as its provider can prefill
//! it (ADR 0556). A *prefill beat* sends it again.
//!
//! Memory only. A restart loses it, so a waiting thread pays one rewrite
//! rather than beat from a request rebuilt on a guess. The store stays
//! bounded: one entry per thread, replaced at its next call, dropped at
//! give-up, and pruned once the thread waits on nothing or its cache is cold.
//!
//! It also keeps the *first-beat guard*'s verdicts: a provider whose first
//! beat failed is unable to prefill until the engine restarts. And it caps
//! how many beats are on their way at once.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use uuid::Uuid;

use crate::engine::cache_ttl::CacheTtl;
use crate::llm::prefill::PrefillPair;
use crate::llm::ProviderKind;

/// What the keep-alive knows of a Lucidos Agent thread's last request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Remembered {
    /// A pair a beat can send, to a cache of this lifetime.
    Pair(CacheTtl),
    /// The provider cannot prefill the last request.
    Uncacheable,
    /// Nothing: the engine restarted since the last request, or the store
    /// let it go.
    Nothing,
}

/// The most beats on their way at once. A fifth waits for the next tick.
pub(crate) const MAX_BEATS_IN_FLIGHT: usize = 4;

#[derive(Default)]
pub(crate) struct RememberedRequests {
    by_thread: Mutex<HashMap<Uuid, Entry>>,
    providers: Mutex<HashMap<ProviderKind, Verdict>>,
    in_flight: AtomicUsize,
}

/// What the first-beat guard learned of a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// A beat read and wrote nothing, so later beats skip the guard.
    Proven,
    /// Its first beat failed. No thread on it beats until a restart.
    Unable,
}

/// One claimed beat slot, given back on drop.
pub(crate) struct BeatSlot<'a>(&'a AtomicUsize);

impl Drop for BeatSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Entry {
    memory: Memory,
    /// When the cache entry last refreshed: the request, or the last beat.
    refreshed: Instant,
}

enum Memory {
    Pair(Box<PrefillPair>, CacheTtl),
    Uncacheable,
}

impl RememberedRequests {
    /// Remember the request a thread just made, replacing the one before.
    /// `None` means its provider could not prefill it.
    pub(crate) fn remember(&self, thread_id: Uuid, pair: Option<PrefillPair>, now: Instant) {
        let memory = match pair.and_then(|pair| Some((cache_ttl_of(pair.stable.provider?), pair))) {
            Some((ttl, pair)) => Memory::Pair(Box::new(pair), ttl),
            None => Memory::Uncacheable,
        };
        self.lock().insert(
            thread_id,
            Entry {
                memory,
                refreshed: now,
            },
        );
    }

    pub(crate) fn state(&self, thread_id: Uuid) -> Remembered {
        let state = match self.lock().get(&thread_id).map(|entry| &entry.memory) {
            Some(Memory::Pair(pair, ttl)) => pair.stable.provider.map(|p| (p, *ttl)),
            Some(Memory::Uncacheable) => return Remembered::Uncacheable,
            None => return Remembered::Nothing,
        };
        match state {
            Some((provider, ttl)) if !self.is_unable(provider) => Remembered::Pair(ttl),
            _ => Remembered::Uncacheable,
        }
    }

    /// The pair a beat on `thread_id` sends, unless its provider is unable.
    pub(crate) fn pair(&self, thread_id: Uuid) -> Option<PrefillPair> {
        match &self.lock().get(&thread_id)?.memory {
            Memory::Pair(pair, _) if !self.is_unable(pair.stable.provider?) => {
                Some(PrefillPair::clone(pair))
            }
            _ => None,
        }
    }

    /// A beat read the thread's entry at `at`, so its lifetime starts again.
    pub(crate) fn refreshed(&self, thread_id: Uuid, at: Instant) {
        if let Some(entry) = self.lock().get_mut(&thread_id) {
            entry.refreshed = at;
        }
    }

    /// Whether the guard still judges `provider`'s next beat: no beat on it
    /// has read and written nothing yet.
    pub(crate) fn unproven(&self, provider: ProviderKind) -> bool {
        self.verdicts().get(&provider).is_none()
    }

    pub(crate) fn mark_proven(&self, provider: ProviderKind) {
        self.verdicts().entry(provider).or_insert(Verdict::Proven);
    }

    pub(crate) fn mark_unable(&self, provider: ProviderKind) {
        self.verdicts().insert(provider, Verdict::Unable);
    }

    fn is_unable(&self, provider: ProviderKind) -> bool {
        self.verdicts().get(&provider) == Some(&Verdict::Unable)
    }

    /// One of the [`MAX_BEATS_IN_FLIGHT`] slots, or `None` when all are taken.
    pub(crate) fn claim_slot(&self) -> Option<BeatSlot<'_>> {
        self.in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_BEATS_IN_FLIGHT).then_some(n + 1)
            })
            .ok()
            .map(|_| BeatSlot(&self.in_flight))
    }

    pub(crate) fn forget(&self, thread_id: Uuid) {
        self.lock().remove(&thread_id);
    }

    /// Keep only the threads in `waiting` whose cache is still warm at
    /// `now`. A pair past its lifetime would make a beat write, not read.
    pub(crate) fn prune(&self, waiting: &HashSet<Uuid>, now: Instant) {
        self.lock().retain(|thread_id, entry| {
            waiting.contains(thread_id)
                && match entry.memory {
                    Memory::Pair(_, ttl) => {
                        now.duration_since(entry.refreshed).as_secs_f64() < ttl.lifetime_secs()
                    }
                    Memory::Uncacheable => true,
                }
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Entry>> {
        self.by_thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn verdicts(&self) -> std::sync::MutexGuard<'_, HashMap<ProviderKind, Verdict>> {
        self.providers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The lifetime a prefill keeps warm on `provider`. OpenAI keeps an entry 30
/// minutes at least. Every Claude route writes the 5-minute cache.
fn cache_ttl_of(provider: ProviderKind) -> CacheTtl {
    match provider {
        ProviderKind::OpenAi => CacheTtl::ThirtyMinutes,
        _ => CacheTtl::FiveMinutes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn pair(provider: Option<ProviderKind>) -> PrefillPair {
        let body = crate::llm::prefill::PrefillBody {
            provider,
            model: "m".into(),
            reasoning_effort: None,
            output_cap: Some(0),
            reply: crate::llm::prefill::ReplyShape::Json,
            wire_model: "m".into(),
            betas: Vec::new(),
            body: bytes::Bytes::from_static(b"{}"),
        };
        PrefillPair {
            stable: body.clone(),
            whole: body,
        }
    }

    fn waiting(threads: &[Uuid]) -> HashSet<Uuid> {
        threads.iter().copied().collect()
    }

    #[test]
    fn a_thread_with_no_request_remembers_nothing() {
        assert_eq!(
            RememberedRequests::default().state(Uuid::new_v4()),
            Remembered::Nothing
        );
    }

    #[test]
    fn the_next_call_replaces_the_last() {
        let store = RememberedRequests::default();
        let thread = Uuid::new_v4();
        let now = Instant::now();
        store.remember(thread, Some(pair(Some(ProviderKind::Vertex))), now);
        assert_eq!(store.state(thread), Remembered::Pair(CacheTtl::FiveMinutes));
        store.remember(thread, None, now);
        assert_eq!(store.state(thread), Remembered::Uncacheable);
        store.remember(thread, Some(pair(Some(ProviderKind::OpenAi))), now);
        assert_eq!(
            store.state(thread),
            Remembered::Pair(CacheTtl::ThirtyMinutes)
        );
    }

    /// A pair no router stamped has no backend to go to.
    #[test]
    fn a_pair_with_no_backend_is_uncacheable() {
        let store = RememberedRequests::default();
        let thread = Uuid::new_v4();
        store.remember(thread, Some(pair(None)), Instant::now());
        assert_eq!(store.state(thread), Remembered::Uncacheable);
    }

    #[test]
    fn give_up_forgets_the_thread() {
        let store = RememberedRequests::default();
        let thread = Uuid::new_v4();
        store.remember(
            thread,
            Some(pair(Some(ProviderKind::Anthropic))),
            Instant::now(),
        );
        store.forget(thread);
        assert_eq!(store.state(thread), Remembered::Nothing);
    }

    /// A provider whose first beat failed is unable until a restart. Its
    /// threads read as uncacheable and hand out no pair.
    #[test]
    fn an_unable_provider_beats_no_thread() {
        let store = RememberedRequests::default();
        let (gateway, vertex) = (Uuid::new_v4(), Uuid::new_v4());
        let now = Instant::now();
        store.remember(gateway, Some(pair(Some(ProviderKind::ClaudeGateway))), now);
        store.remember(vertex, Some(pair(Some(ProviderKind::Vertex))), now);
        assert!(store.unproven(ProviderKind::ClaudeGateway));

        store.mark_unable(ProviderKind::ClaudeGateway);
        assert_eq!(store.state(gateway), Remembered::Uncacheable);
        assert!(store.pair(gateway).is_none());
        assert!(store.pair(vertex).is_some());

        store.mark_proven(ProviderKind::Vertex);
        assert!(!store.unproven(ProviderKind::Vertex));
        store.mark_proven(ProviderKind::ClaudeGateway);
        assert_eq!(
            store.state(gateway),
            Remembered::Uncacheable,
            "a later good verdict never overturns the guard"
        );
    }

    #[test]
    fn a_fifth_beat_waits_for_a_slot() {
        let store = RememberedRequests::default();
        let slots: Vec<_> = (0..MAX_BEATS_IN_FLIGHT)
            .map(|_| store.claim_slot().expect("a free slot"))
            .collect();
        assert!(store.claim_slot().is_none());
        drop(slots);
        assert!(store.claim_slot().is_some());
    }

    #[test]
    fn a_beat_restarts_the_entry_lifetime() {
        let store = RememberedRequests::default();
        let thread = Uuid::new_v4();
        let then = Instant::now();
        store.remember(thread, Some(pair(Some(ProviderKind::Anthropic))), then);
        store.refreshed(thread, then + Duration::from_secs(270));
        store.prune(&waiting(&[thread]), then + Duration::from_secs(400));
        assert_ne!(store.state(thread), Remembered::Nothing);
    }

    #[test]
    fn a_thread_that_waits_on_nothing_is_pruned() {
        let store = RememberedRequests::default();
        let (kept, idle) = (Uuid::new_v4(), Uuid::new_v4());
        let now = Instant::now();
        for thread in [kept, idle] {
            store.remember(thread, Some(pair(Some(ProviderKind::Anthropic))), now);
        }
        store.prune(&waiting(&[kept]), now);
        assert_eq!(store.state(kept), Remembered::Pair(CacheTtl::FiveMinutes));
        assert_eq!(store.state(idle), Remembered::Nothing);
    }

    #[test]
    fn a_pair_past_its_cache_lifetime_is_pruned() {
        let store = RememberedRequests::default();
        let (claude, openai) = (Uuid::new_v4(), Uuid::new_v4());
        let then = Instant::now();
        store.remember(claude, Some(pair(Some(ProviderKind::Anthropic))), then);
        store.remember(openai, Some(pair(Some(ProviderKind::OpenAi))), then);
        let all = waiting(&[claude, openai]);

        store.prune(&all, then + Duration::from_secs(299));
        assert_ne!(store.state(claude), Remembered::Nothing);
        store.prune(&all, then + Duration::from_secs(300));
        assert_eq!(store.state(claude), Remembered::Nothing);
        assert_ne!(store.state(openai), Remembered::Nothing, "30 minutes");
        store.prune(&all, then + Duration::from_secs(1800));
        assert_eq!(store.state(openai), Remembered::Nothing);
    }
}
