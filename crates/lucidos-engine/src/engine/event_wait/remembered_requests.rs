use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::engine::cache_ttl::CacheTtl;
use crate::llm::prefill::PrefillPair;
use crate::llm::ProviderKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Remembered {
    Pair,
    Uncacheable,
    ProviderPaused,
    Missed,
    Expired,
    GaveUp,
    Paused { until: DateTime<Utc> },
    Nothing,
}

pub(crate) const MAX_BEATS_IN_FLIGHT: usize = 4;

pub(crate) const MISSES_TO_PAUSE: u32 = 3;

pub(crate) const PAUSE_AFTER_MISSES: chrono::Duration = chrono::Duration::hours(1);

#[derive(Default)]
pub struct RememberedRequests {
    by_thread: Mutex<HashMap<Uuid, Entry>>,
    providers: Mutex<HashMap<ProviderKind, ProviderRecord>>,
    in_flight: AtomicUsize,
}

#[derive(Debug, Default)]
struct ProviderRecord {
    verdict: Option<Verdict>,
    misses: u32,
    paused_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Proven,
    /// INVARIANT: It refused its first beat, or a beat wrote output. No thread on it
    /// beats until a restart.
    Unable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Able,
    Unable,
    Paused { until: DateTime<Utc> },
}

impl ProviderRecord {
    fn standing(&self, now: DateTime<Utc>) -> Standing {
        match (self.verdict, self.paused_until) {
            (Some(Verdict::Unable), _) => Standing::Unable,
            (_, Some(until)) if now < until => Standing::Paused { until },
            _ => Standing::Able,
        }
    }
}

pub(crate) struct BeatSlot<'a>(&'a AtomicUsize);

impl Drop for BeatSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Entry {
    memory: Memory,
    refreshed: Instant,
}

enum Memory {
    Pair(Arc<PrefillPair>),
    Uncacheable,
    Ended(ProviderKind, Ending),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Missed,
    Expired,
    GaveUp,
}

impl Memory {
    fn pair_provider(&self) -> Option<ProviderKind> {
        match self {
            Memory::Pair(pair) => pair.stable.provider,
            Memory::Uncacheable | Memory::Ended(..) => None,
        }
    }

    fn is(&self, sent: &Arc<PrefillPair>) -> bool {
        matches!(self, Memory::Pair(pair) if Arc::ptr_eq(pair, sent))
    }
}

impl RememberedRequests {
    pub fn remember(&self, thread_id: Uuid, pair: Option<PrefillPair>, now: Instant) {
        let memory = match pair.filter(|pair| pair.stable.provider.is_some()) {
            Some(pair) => Memory::Pair(Arc::new(pair)),
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

    /// INVARIANT: The thread's state at `now`. A provider's standing outranks the
    /// thread's own memory: an unable or paused provider says why first.
    pub(crate) fn state(&self, thread_id: Uuid, now: DateTime<Utc>) -> Remembered {
        let (provider, ending) = match self.lock().get(&thread_id).map(|entry| &entry.memory) {
            Some(Memory::Pair(pair)) => match pair.stable.provider {
                Some(provider) => (provider, None),
                None => return Remembered::Uncacheable,
            },
            Some(Memory::Ended(provider, ending)) => (*provider, Some(*ending)),
            Some(Memory::Uncacheable) => return Remembered::Uncacheable,
            None => return Remembered::Nothing,
        };
        match (self.standing(provider, now), ending) {
            (Standing::Unable, _) => Remembered::ProviderPaused,
            (Standing::Paused { until }, _) => Remembered::Paused { until },
            (Standing::Able, None) => Remembered::Pair,
            (Standing::Able, Some(Ending::Missed)) => Remembered::Missed,
            (Standing::Able, Some(Ending::Expired)) => Remembered::Expired,
            (Standing::Able, Some(Ending::GaveUp)) => Remembered::GaveUp,
        }
    }

    pub(crate) fn pair(&self, thread_id: Uuid, now: DateTime<Utc>) -> Option<Arc<PrefillPair>> {
        let pair = match &self.lock().get(&thread_id)?.memory {
            Memory::Pair(pair) => Arc::clone(pair),
            Memory::Uncacheable | Memory::Ended(..) => return None,
        };
        (self.standing(pair.stable.provider?, now) == Standing::Able).then_some(pair)
    }

    pub(crate) fn refreshed(&self, thread_id: Uuid, sent: &Arc<PrefillPair>, at: Instant) {
        if let Some(entry) = self.lock().get_mut(&thread_id) {
            if entry.memory.is(sent) {
                entry.refreshed = at;
            }
        }
    }

    pub(crate) fn unproven(&self, provider: ProviderKind) -> bool {
        self.providers()
            .get(&provider)
            .is_none_or(|record| record.verdict.is_none())
    }

    pub(crate) fn mark_proven(&self, provider: ProviderKind) {
        let mut providers = self.providers();
        let record = providers.entry(provider).or_default();
        record.verdict.get_or_insert(Verdict::Proven);
        record.misses = 0;
        record.paused_until = None;
    }

    pub(crate) fn mark_unable(&self, provider: ProviderKind) {
        self.providers().entry(provider).or_default().verdict = Some(Verdict::Unable);
    }

    pub(crate) fn mark_missed(
        &self,
        thread_id: Uuid,
        sent: &Arc<PrefillPair>,
        provider: ProviderKind,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        self.end_run(thread_id, sent);
        let mut providers = self.providers();
        let record = providers.entry(provider).or_default();
        record.misses += 1;
        if record.misses < MISSES_TO_PAUSE {
            return None;
        }
        let until = now + PAUSE_AFTER_MISSES;
        record.paused_until = Some(until);
        Some(until)
    }

    pub(crate) fn end_run(&self, thread_id: Uuid, sent: &Arc<PrefillPair>) {
        self.end(thread_id, Ending::Missed, |memory| memory.is(sent));
    }

    pub(crate) fn give_up(&self, thread_id: Uuid) {
        self.end(thread_id, Ending::GaveUp, |_| true);
    }

    fn end(&self, thread_id: Uuid, ending: Ending, ends: impl FnOnce(&Memory) -> bool) {
        if let Some(entry) = self.lock().get_mut(&thread_id) {
            if let Some(provider) = entry.memory.pair_provider().filter(|_| ends(&entry.memory)) {
                entry.memory = Memory::Ended(provider, ending);
            }
        }
    }

    fn standing(&self, provider: ProviderKind, now: DateTime<Utc>) -> Standing {
        self.providers()
            .get(&provider)
            .map_or(Standing::Able, |record| record.standing(now))
    }

    pub(crate) fn claim_slot(&self) -> Option<BeatSlot<'_>> {
        self.in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_BEATS_IN_FLIGHT).then_some(n + 1)
            })
            .ok()
            .map(|_| BeatSlot(&self.in_flight))
    }

    pub(crate) fn prune(&self, waiting: &HashMap<Uuid, CacheTtl>, now: Instant) {
        self.lock().retain(|thread_id, entry| {
            let Some(ttl) = waiting.get(thread_id) else {
                return false;
            };
            let expired = ttl.lifetime().is_some_and(|lifetime| {
                now.duration_since(entry.refreshed).as_secs_f64() >= lifetime.lifetime_secs()
            });
            if let Some(provider) = entry.memory.pair_provider().filter(|_| expired) {
                entry.memory = Memory::Ended(provider, Ending::Expired);
            }
            true
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Entry>> {
        self.by_thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn providers(&self) -> std::sync::MutexGuard<'_, HashMap<ProviderKind, ProviderRecord>> {
        self.providers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cache_ttl::CacheLifetime;
    use std::time::Duration;

    fn pair(provider: Option<ProviderKind>) -> PrefillPair {
        let body = crate::llm::prefill::PrefillBody::for_test(provider, "m");
        PrefillPair {
            stable: body.clone(),
            whole: body,
        }
    }

    const FIVE_MINUTES: CacheTtl = CacheTtl::Reported(CacheLifetime::FiveMinutes);
    const GATEWAY: ProviderKind = ProviderKind::ClaudeGateway;

    fn waiting(threads: &[(Uuid, CacheTtl)]) -> HashMap<Uuid, CacheTtl> {
        threads.iter().copied().collect()
    }

    fn noon() -> DateTime<Utc> {
        "2026-01-01T12:00:00Z".parse().unwrap()
    }

    fn remembering(store: &RememberedRequests, provider: ProviderKind) -> Uuid {
        let thread = Uuid::new_v4();
        store.remember(thread, Some(pair(Some(provider))), Instant::now());
        thread
    }

    fn miss(
        store: &RememberedRequests,
        thread: Uuid,
        provider: ProviderKind,
        at: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let sent = store.pair(thread, at).expect("a pair to beat");
        store.mark_missed(thread, &sent, provider, at)
    }

    #[test]
    fn a_thread_with_no_request_remembers_nothing() {
        assert_eq!(
            RememberedRequests::default().state(Uuid::new_v4(), noon()),
            Remembered::Nothing
        );
    }

    #[test]
    fn the_next_call_replaces_the_last() {
        let store = RememberedRequests::default();
        let thread = Uuid::new_v4();
        let now = Instant::now();
        store.remember(thread, Some(pair(Some(ProviderKind::Vertex))), now);
        assert_eq!(store.state(thread, noon()), Remembered::Pair);
        store.remember(thread, None, now);
        assert_eq!(store.state(thread, noon()), Remembered::Uncacheable);
        store.remember(thread, Some(pair(Some(ProviderKind::OpenAi))), now);
        assert_eq!(store.state(thread, noon()), Remembered::Pair);
    }

    #[test]
    fn a_pair_with_no_backend_is_uncacheable() {
        let store = RememberedRequests::default();
        let thread = Uuid::new_v4();
        store.remember(thread, Some(pair(None)), Instant::now());
        assert_eq!(store.state(thread, noon()), Remembered::Uncacheable);
    }

    #[test]
    fn give_up_releases_the_pair_and_keeps_its_cause() {
        let store = RememberedRequests::default();
        let thread = remembering(&store, ProviderKind::Anthropic);
        store.give_up(thread);
        assert_eq!(store.state(thread, noon()), Remembered::GaveUp);
        assert!(store.pair(thread, noon()).is_none());
        store.prune(&waiting(&[]), Instant::now());
        assert_eq!(store.state(thread, noon()), Remembered::Nothing);
    }

    #[test]
    fn give_up_preserves_a_missed_or_expired_cause() {
        let store = RememberedRequests::default();
        let missed = remembering(&store, ProviderKind::Vertex);
        miss(&store, missed, ProviderKind::Vertex, noon());
        let expired = remembering(&store, ProviderKind::Vertex);
        store.prune(
            &waiting(&[(missed, FIVE_MINUTES), (expired, FIVE_MINUTES)]),
            Instant::now() + Duration::from_secs(300),
        );
        for (thread, cause) in [(missed, Remembered::Missed), (expired, Remembered::Expired)] {
            store.give_up(thread);
            assert_eq!(store.state(thread, noon()), cause);
        }
    }

    #[test]
    fn an_unable_provider_beats_no_thread() {
        let store = RememberedRequests::default();
        let gateway = remembering(&store, GATEWAY);
        let vertex = remembering(&store, ProviderKind::Vertex);
        assert!(store.unproven(GATEWAY));

        store.mark_unable(GATEWAY);
        assert_eq!(store.state(gateway, noon()), Remembered::ProviderPaused);
        assert!(store.pair(gateway, noon()).is_none());
        assert!(store.pair(vertex, noon()).is_some());
        let another = remembering(&store, GATEWAY);
        assert_eq!(store.state(another, noon()), Remembered::ProviderPaused);
        store.remember(another, None, Instant::now());
        assert_eq!(store.state(another, noon()), Remembered::Uncacheable);
        store.remember(another, Some(pair(None)), Instant::now());
        assert_eq!(store.state(another, noon()), Remembered::Uncacheable);

        store.mark_proven(ProviderKind::Vertex);
        assert!(!store.unproven(ProviderKind::Vertex));
        store.mark_proven(GATEWAY);
        assert_eq!(
            store.state(gateway, noon()),
            Remembered::ProviderPaused,
            "a later good verdict never overturns the guard"
        );
    }

    #[test]
    fn a_missed_beat_ends_only_its_own_run() {
        let store = RememberedRequests::default();
        let (missed, other) = (remembering(&store, GATEWAY), remembering(&store, GATEWAY));
        assert_eq!(miss(&store, missed, GATEWAY, noon()), None);
        assert_eq!(store.state(missed, noon()), Remembered::Missed);
        assert!(store.pair(missed, noon()).is_none());
        assert_eq!(store.state(other, noon()), Remembered::Pair);
        assert!(store.pair(other, noon()).is_some());

        store.remember(missed, Some(pair(Some(GATEWAY))), Instant::now());
        assert_eq!(
            store.state(missed, noon()),
            Remembered::Pair,
            "the next request starts again"
        );
    }

    #[test]
    fn a_stale_beat_spares_a_request_made_while_it_was_out() {
        let store = RememberedRequests::default();
        let now = Instant::now();
        let thread = Uuid::new_v4();
        store.remember(thread, Some(pair(Some(GATEWAY))), now);
        let sent = store.pair(thread, noon()).unwrap();
        store.remember(thread, Some(pair(Some(GATEWAY))), now);
        store.mark_missed(thread, &sent, GATEWAY, noon());
        assert_eq!(store.state(thread, noon()), Remembered::Pair);
        store.refreshed(thread, &sent, now + Duration::from_secs(270));
        store.prune(
            &waiting(&[(thread, FIVE_MINUTES)]),
            now + Duration::from_secs(300),
        );
        assert_eq!(
            store.state(thread, noon()),
            Remembered::Expired,
            "the stale read never extended the new request"
        );
    }

    #[test]
    fn misses_in_a_row_pause_the_provider_for_an_hour() {
        let store = RememberedRequests::default();
        let other = remembering(&store, GATEWAY);
        let miss = |at: DateTime<Utc>| miss(&store, remembering(&store, GATEWAY), GATEWAY, at);
        let until = noon() + PAUSE_AFTER_MISSES;

        assert_eq!(miss(noon()), None);
        store.mark_proven(GATEWAY);
        assert_eq!(miss(noon()), None, "a read reset the streak");
        assert_eq!(miss(noon()), None, "two in a row");
        assert_eq!(store.state(other, noon()), Remembered::Pair);
        assert_eq!(miss(noon()), Some(until), "the third in a row pauses");

        let second = chrono::Duration::seconds(1);
        assert_eq!(
            store.state(other, until - second),
            Remembered::Paused { until }
        );
        assert!(store.pair(other, until - second).is_none());
        assert_eq!(store.state(other, until), Remembered::Pair, "it expired");
        assert!(store.pair(other, until).is_some());

        assert_eq!(
            miss(until),
            Some(until + PAUSE_AFTER_MISSES),
            "one more write after the pause pauses again"
        );
        store.mark_proven(GATEWAY);
        assert_eq!(
            store.state(other, until),
            Remembered::Pair,
            "a read ends it"
        );
    }

    #[test]
    fn a_pause_expiry_never_clears_an_unable_provider() {
        let store = RememberedRequests::default();
        let thread = remembering(&store, GATEWAY);
        for _ in 0..MISSES_TO_PAUSE {
            miss(&store, remembering(&store, GATEWAY), GATEWAY, noon());
        }
        store.mark_unable(GATEWAY);
        let later = noon() + PAUSE_AFTER_MISSES * 2;
        assert_eq!(store.state(thread, later), Remembered::ProviderPaused);
        assert!(store.pair(thread, later).is_none());
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
        let sent = store.pair(thread, noon()).unwrap();
        store.refreshed(thread, &sent, then + Duration::from_secs(270));
        store.prune(
            &waiting(&[(thread, FIVE_MINUTES)]),
            then + Duration::from_secs(400),
        );
        assert_eq!(store.state(thread, noon()), Remembered::Pair);
    }

    #[test]
    fn a_thread_that_waits_on_nothing_is_pruned() {
        let store = RememberedRequests::default();
        let (kept, idle, missed) = (
            remembering(&store, ProviderKind::Anthropic),
            remembering(&store, ProviderKind::Anthropic),
            remembering(&store, ProviderKind::Anthropic),
        );
        miss(&store, missed, ProviderKind::Anthropic, noon());
        store.prune(&waiting(&[(kept, FIVE_MINUTES)]), Instant::now());
        assert_eq!(store.state(kept, noon()), Remembered::Pair);
        assert_eq!(store.state(idle, noon()), Remembered::Nothing);
        assert_eq!(store.state(missed, noon()), Remembered::Nothing);
        assert_eq!(store.lock().len(), 1);
    }

    #[test]
    fn a_pair_past_its_reported_cache_lifetime_expires() {
        let store = RememberedRequests::default();
        let (short, long) = (Uuid::new_v4(), Uuid::new_v4());
        let then = Instant::now();
        store.remember(short, Some(pair(Some(ProviderKind::OpenAi))), then);
        store.remember(long, Some(pair(Some(ProviderKind::Anthropic))), then);
        let all = waiting(&[
            (short, FIVE_MINUTES),
            (long, CacheTtl::Reported(CacheLifetime::ThirtyMinutes)),
        ]);

        store.prune(&all, then + Duration::from_secs(299));
        assert_eq!(store.state(short, noon()), Remembered::Pair);
        store.prune(&all, then + Duration::from_secs(300));
        assert_eq!(store.state(short, noon()), Remembered::Expired);
        assert!(store.pair(short, noon()).is_none());
        assert_eq!(store.state(long, noon()), Remembered::Pair, "30 minutes");
        store.prune(&all, then + Duration::from_secs(1800));
        assert_eq!(store.state(long, noon()), Remembered::Expired);
    }

    #[test]
    fn a_pair_with_an_unknown_ttl_stays_while_its_thread_waits() {
        let store = RememberedRequests::default();
        let thread = Uuid::new_v4();
        let then = Instant::now();
        store.remember(thread, Some(pair(Some(ProviderKind::OpenRouter))), then);
        let day = then + Duration::from_secs(86_400);
        store.prune(&waiting(&[(thread, CacheTtl::Unknown)]), day);
        assert_eq!(store.state(thread, noon()), Remembered::Pair);
        store.prune(&waiting(&[]), day);
        assert_eq!(store.state(thread, noon()), Remembered::Nothing);
    }
}
