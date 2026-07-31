//! In-memory cache of `script_handshake` auth headers + expiry, with a
//! singleflight gate so concurrent first-time requests for the same proxy
//! don't all spawn the script. The gate also handles the failure path
//! correctly: if the leader's script call fails, the FIRST waiting
//! follower becomes the next leader and the rest park on its attempt.
//! K consecutive script failures with N concurrent waiters yield exactly
//! K+1 script invocations, not K*N+1.
//!
//! Cancellation safety: the leader's slot is owned by a `LeaderGuard`
//! whose `Drop` calls `finish(succeeded=false)` if the future is cancelled
//! before the leader explicitly completes. Without this, an axum handler
//! dropped mid-`refresh` (HTTP client disconnected on its own timeout)
//! would leave the slot in the inflight map with `done=false` and
//! `needs_leader=false` — wedging every subsequent caller as a Follower
//! on a watch channel that never fires. The `inflight` map uses
//! `std::sync::Mutex` (not `tokio::sync::Mutex`) so the guard's Drop
//! can release the slot synchronously from any context.

use axum::http::{HeaderName, HeaderValue};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::{watch, RwLock};

#[derive(Clone, Debug)]
pub struct CachedToken {
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub expires_at: Instant,
}

#[derive(Default)]
pub struct ProxyTokenCache {
    entries: RwLock<HashMap<String, CachedToken>>,
    /// `std::sync::Mutex` (not tokio's) so `LeaderGuard::drop` can release
    /// a cancelled leader's slot synchronously without needing an async
    /// context. Critical sections are tiny — hash-map ops + atomics + a
    /// sync `watch::Sender::send_replace` — so blocking the executor
    /// thread for a few hundred nanoseconds is fine.
    inflight: StdMutex<HashMap<String, Arc<InflightSlot>>>,
}

/// One round of leadership. The slot stays in the inflight map across
/// the leader's whole work AND across the leader→next-leader handoff on
/// failure: on Err the leader sets `needs_leader=true` instead of
/// removing the slot, so late-arriving followers always see an existing
/// slot and subscribe — never racing each other to become independent
/// leaders. `watch` (not `Notify`) so subscribers that arrive after the
/// leader's `done=true` send still observe it; `Notify::notify_waiters`
/// drops wakeups for late subscribers.
struct InflightSlot {
    done: watch::Sender<bool>,
    needs_leader: AtomicBool,
}

impl InflightSlot {
    fn new() -> Arc<Self> {
        let (tx, _rx) = watch::channel(false);
        Arc::new(Self {
            done: tx,
            needs_leader: AtomicBool::new(false),
        })
    }
}

enum Outcome {
    Leader(Arc<InflightSlot>),
    Follower(watch::Receiver<bool>),
}

impl ProxyTokenCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached entry IF it exists and hasn't expired.
    pub async fn get(&self, name: &str) -> Option<CachedToken> {
        let entries = self.entries.read().await;
        let entry = entries.get(name)?;
        if entry.expires_at <= Instant::now() {
            return None;
        }
        Some(entry.clone())
    }

    /// Returns the inserted `CachedToken` so callers don't need a
    /// follow-up `get` (and so avoid racing with `invalidate`).
    pub async fn insert(
        &self,
        name: &str,
        headers: Vec<(HeaderName, HeaderValue)>,
        ttl: Duration,
    ) -> CachedToken {
        let token = CachedToken {
            headers,
            expires_at: Instant::now() + ttl,
        };
        self.entries
            .write()
            .await
            .insert(name.to_string(), token.clone());
        token
    }

    pub async fn invalidate(&self, name: &str) {
        self.entries.write().await.remove(name);
    }

    /// Singleflight: returns the cached token (if fresh) or runs `refresh`
    /// to mint one. Concurrent callers for the same `name` collapse into
    /// a single `refresh` per attempt. The returned `bool` is `true` iff
    /// the value came from a pre-existing cache entry on entry to this
    /// call (no `refresh` was triggered for this caller).
    ///
    /// Failure semantics: the leader of each attempt returns its `Err` to
    /// its own caller. Followers wake, observe the failure (cache miss),
    /// and the FIRST one to re-enter `enter` claims the next attempt
    /// (becomes the new leader); the rest park on the new attempt.
    /// K consecutive failures → exactly K+1 calls to `refresh`,
    /// regardless of the number of concurrent followers.
    ///
    /// Cancellation safety: every leader path holds a `LeaderGuard`. If
    /// the future is dropped between acquiring leadership and explicit
    /// completion (axum cancels the handler when the HTTP client gives
    /// up), the guard's Drop calls `finish(succeeded=false)` synchronously
    /// so the slot doesn't strand followers forever.
    pub async fn get_or_refresh<F, Fut, E>(
        &self,
        name: &str,
        refresh: F,
    ) -> Result<(CachedToken, bool), E>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<(Vec<(HeaderName, HeaderValue)>, Duration), E>>,
    {
        if let Some(token) = self.get(name).await {
            return Ok((token, true));
        }
        loop {
            match self.enter(name) {
                Outcome::Leader(slot) => {
                    let guard = LeaderGuard::new(self, name, slot);
                    // Re-check cache: a different leader may have
                    // succeeded between our initial miss and acquiring
                    // leadership. Without this, a tail-end follower can
                    // become a redundant leader after a fast successor
                    // already populated the cache (closes the
                    // success-case stampede window).
                    if let Some(token) = self.get(name).await {
                        guard.complete(true);
                        return Ok((token, false));
                    }
                    match refresh().await {
                        Ok((headers, ttl)) => {
                            let token = self.insert(name, headers, ttl).await;
                            guard.complete(true);
                            return Ok((token, false));
                        }
                        Err(e) => {
                            guard.complete(false);
                            return Err(e);
                        }
                    }
                }
                Outcome::Follower(mut rx) => {
                    // Subscribed under the inflight Mutex in `enter`, so
                    // we cannot miss the leader's `done=true` even if it
                    // fires immediately.
                    let _ = rx.wait_for(|done| *done).await;
                    if let Some(token) = self.get(name).await {
                        return Ok((token, false));
                    }
                    // Cache miss → leader failed (or was cancelled).
                    // Loop; first caller in `enter` claims the next round.
                    continue;
                }
            }
        }
    }

    /// Subscribe-or-claim is atomic against `finish` — both run under
    /// the inflight Mutex. Sync because the critical section is tiny
    /// and `LeaderGuard::drop` (also synchronous) shares this path.
    fn enter(&self, name: &str) -> Outcome {
        let mut inflight = self.inflight.lock().expect("inflight mutex poisoned");
        if let Some(existing) = inflight.get(name).cloned() {
            // Previous round failed (or was cancelled); first swap-winner
            // claims the next round. Replace with a fresh slot so new
            // followers subscribe to a Pending channel, not the
            // already-fired one.
            if existing.needs_leader.swap(false, Ordering::SeqCst) {
                let new_slot = InflightSlot::new();
                inflight.insert(name.to_string(), new_slot.clone());
                return Outcome::Leader(new_slot);
            }
            return Outcome::Follower(existing.done.subscribe());
        }
        let slot = InflightSlot::new();
        inflight.insert(name.to_string(), slot.clone());
        Outcome::Leader(slot)
    }

    fn finish(&self, name: &str, slot: &Arc<InflightSlot>, succeeded: bool) {
        {
            let mut inflight = self.inflight.lock().expect("inflight mutex poisoned");
            let still_ours = inflight.get(name).is_some_and(|cur| Arc::ptr_eq(cur, slot));
            if still_ours {
                if succeeded {
                    inflight.remove(name);
                } else {
                    slot.needs_leader.store(true, Ordering::SeqCst);
                }
            }
        }
        // Always wake THIS slot's subscribers — they may have subscribed
        // before a successor replaced us in inflight.
        let _ = slot.done.send_replace(true);
    }
}

/// RAII handle for the leader's hold on an `InflightSlot`. The leader
/// must call `complete(succeeded)` on the happy path; if the future is
/// cancelled (axum drops the handler when the HTTP client disconnects),
/// `Drop` calls `finish` with `succeeded=false` so the slot doesn't
/// leak. Without this, a cancelled leader leaves the slot in the map
/// with `done=false` and `needs_leader=false`, and every later caller
/// subscribes as a Follower to a watch channel that will never fire —
/// observed in production as the "comfort-cloud hung for 21 hours"
/// regression after a transient network failure.
struct LeaderGuard<'a> {
    cache: &'a ProxyTokenCache,
    name: String,
    slot: Arc<InflightSlot>,
    completed: bool,
}

impl<'a> LeaderGuard<'a> {
    fn new(cache: &'a ProxyTokenCache, name: &str, slot: Arc<InflightSlot>) -> Self {
        Self {
            cache,
            name: name.to_string(),
            slot,
            completed: false,
        }
    }

    fn complete(mut self, succeeded: bool) {
        self.cache.finish(&self.name, &self.slot, succeeded);
        self.completed = true;
    }
}

impl<'a> Drop for LeaderGuard<'a> {
    fn drop(&mut self) {
        if !self.completed {
            // Cancelled before explicit completion → treat as a failure
            // round so the next caller can swap in as the new leader.
            self.cache.finish(&self.name, &self.slot, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn header(n: &str, v: &str) -> (HeaderName, HeaderValue) {
        (
            HeaderName::from_bytes(n.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        )
    }

    fn ok_headers() -> Vec<(HeaderName, HeaderValue)> {
        vec![header("authorization", "Bearer ok")]
    }

    #[tokio::test]
    async fn miss_when_empty() {
        let c = ProxyTokenCache::new();
        assert!(c.get("x").await.is_none());
    }

    #[tokio::test]
    async fn hit_when_inserted_and_not_expired() {
        let c = ProxyTokenCache::new();
        c.insert(
            "x",
            vec![header("Authorization", "Bearer abc")],
            Duration::from_secs(60),
        )
        .await;
        let entry = c.get("x").await.expect("expected hit");
        assert_eq!(entry.headers.len(), 1);
        assert_eq!(entry.headers[0].0.as_str(), "authorization");
        assert_eq!(entry.headers[0].1.to_str().unwrap(), "Bearer abc");
    }

    #[tokio::test]
    async fn miss_when_expired() {
        let c = ProxyTokenCache::new();
        c.insert("x", vec![header("X", "y")], Duration::from_millis(0))
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(c.get("x").await.is_none());
    }

    #[tokio::test]
    async fn invalidate_removes_entry() {
        let c = ProxyTokenCache::new();
        c.insert("x", vec![header("X", "y")], Duration::from_secs(60))
            .await;
        c.invalidate("x").await;
        assert!(c.get("x").await.is_none());
    }

    #[tokio::test]
    async fn get_or_refresh_cache_hit_short_circuits_refresh() {
        let c = ProxyTokenCache::new();
        c.insert("x", ok_headers(), Duration::from_secs(60)).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = calls.clone();

        let (token, was_hit) = c
            .get_or_refresh::<_, _, ()>("x", || {
                let calls = calls_in.clone();
                async move {
                    calls.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok((ok_headers(), Duration::from_secs(60)))
                }
            })
            .await
            .unwrap();

        assert!(was_hit, "cache hit must report was_hit=true");
        assert_eq!(token.headers.len(), 1);
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            0,
            "refresh must NOT run on a cache hit"
        );
    }

    #[tokio::test]
    async fn get_or_refresh_cache_miss_invokes_refresh_once() {
        let c = ProxyTokenCache::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = calls.clone();

        let (token, was_hit) = c
            .get_or_refresh::<_, _, ()>("x", || {
                let calls = calls_in.clone();
                async move {
                    calls.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok((ok_headers(), Duration::from_secs(60)))
                }
            })
            .await
            .unwrap();

        assert!(!was_hit, "minted token should report was_hit=false");
        assert_eq!(token.headers[0].1, "Bearer ok");
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        // Inflight slot should be removed after success.
        assert!(c.inflight.lock().unwrap().get("x").is_none());
    }

    #[tokio::test]
    async fn get_or_refresh_propagates_error_to_caller() {
        let c = ProxyTokenCache::new();
        let result: Result<_, &'static str> = c.get_or_refresh("x", || async { Err("boom") }).await;
        assert_eq!(result.unwrap_err(), "boom");
    }

    /// Spawn `n` followers all calling `get_or_refresh(name)`; the mock
    /// upstream fails the first `k` times then succeeds. With
    /// `extra_await=true` the script yields once before deciding,
    /// exposing the failure→handoff window. Returns the count of
    /// upstream invocations and the per-follower results.
    async fn run_stampede_scenario(
        name: &'static str,
        n: usize,
        k: usize,
        extra_await: bool,
    ) -> (usize, Vec<Result<(CachedToken, bool), String>>) {
        let cache = Arc::new(ProxyTokenCache::new());
        let upstream_calls = Arc::new(AtomicUsize::new(0));
        let fails_remaining = Arc::new(AtomicUsize::new(k));
        let barrier = Arc::new(tokio::sync::Barrier::new(n));

        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let cache = cache.clone();
            let upstream_calls = upstream_calls.clone();
            let fails_remaining = fails_remaining.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                cache
                    .get_or_refresh::<_, _, String>(name, || {
                        let upstream_calls = upstream_calls.clone();
                        let fails_remaining = fails_remaining.clone();
                        async move {
                            upstream_calls.fetch_add(1, AtomicOrdering::SeqCst);
                            if extra_await {
                                tokio::task::yield_now().await;
                            }
                            if fails_remaining.load(AtomicOrdering::SeqCst) > 0 {
                                fails_remaining.fetch_sub(1, AtomicOrdering::SeqCst);
                                Err("script failed".to_string())
                            } else {
                                Ok((ok_headers(), Duration::from_secs(60)))
                            }
                        }
                    })
                    .await
            }));
        }

        let mut results = Vec::with_capacity(n);
        for h in handles {
            results.push(
                h.await
                    .expect("task panicked or hung — likely a stranded follower"),
            );
        }
        (upstream_calls.load(AtomicOrdering::SeqCst), results)
    }

    /// Core property: K leader failures × N concurrent followers ⇒
    /// exactly K+1 upstream calls — one per round, not K*N+1. Worst
    /// case is multi-threaded + instant-failing script: under the prior
    /// Notify singleflight this produced 5–6 calls instead of 4.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn k_failures_with_n_followers_yield_k_plus_1_upstream_calls() {
        let (n, k) = (64usize, 3usize);
        let (calls, results) = run_stampede_scenario("x", n, k, false).await;

        assert_eq!(
            calls,
            k + 1,
            "expected K+1={} calls, got {calls}; a stampede would be ~K*N+1={}",
            k + 1,
            k * n + 1
        );
        let oks = results.iter().filter(|r| r.is_ok()).count();
        let errs = results.iter().filter(|r| r.is_err()).count();
        assert_eq!(
            errs, k,
            "expected K={k} Errs (one per failed round), got {errs}"
        );
        assert_eq!(oks, n - k);
    }

    /// Many failure rounds across many trials — flakes show up here.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn no_stampede_under_repeated_failure_rounds() {
        let k = 5usize;
        for trial in 0..10 {
            let (calls, _) = run_stampede_scenario("y", 32, k, false).await;
            assert_eq!(
                calls,
                k + 1,
                "trial {trial}: expected K+1={} calls, got {calls}",
                k + 1
            );
        }
    }

    /// Followers subscribed to a slot that fails AND gets replaced by a
    /// successor must still wake (the orphaned slot's `done=true` must
    /// reach them so they re-enter and subscribe to the new round).
    /// Regression guard for `finish` skipping the wake when the slot is
    /// no longer current.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn followers_subscribed_to_orphaned_slot_still_wake() {
        let k = 2usize;
        let (calls, _) = run_stampede_scenario("z", 16, k, true).await;
        assert_eq!(calls, k + 1);
    }

    /// Regression test for the "comfort-cloud hangs for 21 hours after a
    /// transient network failure" bug. A leader whose `get_or_refresh`
    /// future is cancelled mid-`refresh` (axum drops the handler when the
    /// HTTP client disconnects) MUST release its inflight slot so the next
    /// caller can mint a new token. Before the Drop-guard fix, the slot
    /// stayed in the inflight map with `done=false` and `needs_leader=false`,
    /// so every subsequent caller subscribed as a Follower to a watch
    /// channel that would never fire — wedging the proxy until restart.
    #[tokio::test]
    async fn cancelled_leader_does_not_strand_followers() {
        use tokio::sync::Notify;

        let cache = Arc::new(ProxyTokenCache::new());

        // Block the first refresh on a Notify so we can guarantee the
        // leader is inside `refresh()` when we cancel it.
        let leader_inside_refresh = Arc::new(Notify::new());
        let leader_inside_refresh_in = leader_inside_refresh.clone();

        let cache_for_leader = cache.clone();
        let leader = tokio::spawn(async move {
            cache_for_leader
                .get_or_refresh::<_, _, &'static str>("x", || {
                    let leader_inside_refresh = leader_inside_refresh_in.clone();
                    async move {
                        leader_inside_refresh.notify_one();
                        // Hang forever — simulates a handshake script
                        // that's stuck on a broken DNS lookup. Real
                        // production code has a 30s timeout in the script
                        // runner; here we don't need one because the test
                        // aborts the join handle.
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                })
                .await
        });

        // Wait until the leader is inside refresh() — so cancellation
        // hits the mid-`refresh` await, which is the bug's window.
        leader_inside_refresh.notified().await;

        // Cancel the leader — same effect as axum dropping the handler
        // when the HTTP client disconnects after its 30s timeout.
        leader.abort();
        let _ = leader.await;

        // Now a fresh caller comes in. Under the bug it sees the leaked
        // slot, subscribes as a Follower, and waits forever. The test's
        // 2-second timeout catches that.
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            cache.get_or_refresh::<_, _, &'static str>("x", || async {
                Ok((ok_headers(), Duration::from_secs(60)))
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "cancelled leader leaked its inflight slot — follower hung forever"
        );
        let (token, was_hit) = result.unwrap().expect("recovery refresh should succeed");
        assert!(
            !was_hit,
            "recovery refresh must mint a new token, not report a hit"
        );
        assert_eq!(token.headers[0].1, "Bearer ok");
    }

    /// Concurrent followers must ALSO recover when the leader is cancelled
    /// mid-refresh — not just a single fresh caller that arrives after the
    /// abort. Followers that subscribed to the doomed slot need to either
    /// (a) be woken when the leader's Drop fires `done=true`, or (b) time
    /// out reasonably so they re-enter and take a new leader slot.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelled_leader_wakes_subscribed_followers() {
        use tokio::sync::{Barrier, Notify};

        let cache = Arc::new(ProxyTokenCache::new());

        let leader_inside_refresh = Arc::new(Notify::new());
        let leader_inside_refresh_in = leader_inside_refresh.clone();
        let cache_for_leader = cache.clone();
        let leader = tokio::spawn(async move {
            cache_for_leader
                .get_or_refresh::<_, _, &'static str>("y", || {
                    let leader_inside_refresh = leader_inside_refresh_in.clone();
                    async move {
                        leader_inside_refresh.notify_one();
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                })
                .await
        });
        leader_inside_refresh.notified().await;

        // Spin up N followers that all park on the leader's slot.
        let n = 8usize;
        let followers_started = Arc::new(Barrier::new(n + 1));
        let mut followers = Vec::with_capacity(n);
        for _ in 0..n {
            let cache = cache.clone();
            let followers_started = followers_started.clone();
            followers.push(tokio::spawn(async move {
                followers_started.wait().await;
                cache
                    .get_or_refresh::<_, _, &'static str>("y", || async {
                        Ok((ok_headers(), Duration::from_secs(60)))
                    })
                    .await
            }));
        }
        followers_started.wait().await;
        // Give followers a moment to subscribe as Followers under the
        // doomed slot (rather than racing with the abort).
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Cancel the leader.
        leader.abort();
        let _ = leader.await;

        // All followers must finish within a bounded window.
        for f in followers {
            let result = tokio::time::timeout(Duration::from_secs(2), f).await;
            let (token, _) = result
                .expect("a follower hung — leader Drop did not wake subscribers")
                .expect("follower task panicked")
                .expect("follower refresh failed");
            assert_eq!(token.headers[0].1, "Bearer ok");
        }
    }

    /// Two distinct names share a single cache without blocking each
    /// other — refresh of one must not stall callers waiting on the
    /// other.
    #[tokio::test]
    async fn different_names_do_not_block_each_other() {
        use tokio::sync::Notify;

        let cache = Arc::new(ProxyTokenCache::new());
        let release_a = Arc::new(Notify::new());

        let cache_a = cache.clone();
        let release_a_in_a = release_a.clone();
        let a = tokio::spawn(async move {
            cache_a
                .get_or_refresh::<_, _, ()>("a", || {
                    let release = release_a_in_a.clone();
                    async move {
                        release.notified().await;
                        Ok((ok_headers(), Duration::from_secs(60)))
                    }
                })
                .await
        });

        // Sibling key 'b' must complete without waiting for 'a'.
        let (token_b, _) = cache
            .get_or_refresh::<_, _, ()>("b", || async {
                Ok((ok_headers(), Duration::from_secs(60)))
            })
            .await
            .unwrap();
        assert_eq!(token_b.headers.len(), 1);

        release_a.notify_one();
        let (token_a, _) = a.await.unwrap().unwrap();
        assert_eq!(token_a.headers.len(), 1);
    }
}
