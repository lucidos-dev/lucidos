use super::common::*;
use super::*;
use std::sync::Arc;
use uuid::Uuid;

#[test]
fn register_thread_creates_fresh_token() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (token, _injection_rx, _guard) = register(&threads, tid);
    assert!(!token.is_cancelled());
    assert!(threads.lock().unwrap().contains_key(&tid));
}

#[test]
fn guard_drop_removes_from_map() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (token, _injection_rx, guard) = register(&threads, tid);
    assert!(threads.lock().unwrap().contains_key(&tid));
    drop(guard);
    assert!(!threads.lock().unwrap().contains_key(&tid));
    // Token is NOT cancelled by guard drop — only removed from map
    assert!(!token.is_cancelled());
}

#[test]
fn guard_drop_does_not_cancel_token() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (token, _injection_rx, guard) = register(&threads, tid);
    drop(guard);
    // Critical: dropping guard must NOT cancel the token
    // This ensures Claude Code sessions aren't killed when guards drop
    assert!(!token.is_cancelled(), "guard drop must not cancel token");
}

#[test]
fn two_wakes_for_one_child_completion_admit_one_run() {
    // The live incident. A child completing produced two independent wakes for
    // its parent: the `EventWaitDelivered` re-entry for the awaited
    // `CodingAgentIdled`, and the fan-in's own `ChildThreadCompleted` drive.
    // Both reached admission with no turn yet registered.
    //
    // The predecessor inserted unconditionally, so both came back owning the
    // thread and two agentic loops ran side by side, every tool call twice.
    // Exactly one may win. The loser coalesces (see the test below).
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let wait_reentry = try_register_thread(&threads, &completions, tid).admitted();
    let fan_in_drive = try_register_thread(&threads, &completions, tid).admitted();

    assert!(wait_reentry.is_some(), "the first wake owns the turn");
    assert!(
        fan_in_drive.is_none(),
        "the second wake must be refused, not handed a second live handle"
    );
    assert_eq!(
        threads.lock().unwrap().len(),
        1,
        "one handle for one thread, always"
    );
}

#[test]
fn admission_is_single_flight_under_real_parallelism() {
    // The same invariant where it actually has to hold: several OS threads
    // asking at once, released together by a barrier. Every one of them saw a
    // free map under the old check-then-insert, and every one of them was
    // handed a run.
    const RACERS: usize = 8;
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();
    let barrier = Arc::new(std::sync::Barrier::new(RACERS));

    let racers: Vec<_> = (0..RACERS)
        .map(|_| {
            let threads = threads.clone();
            let completions = completions.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                try_register_thread(&threads, &completions, tid).admitted()
            })
        })
        .collect();

    // Collected, not dropped inside the racer: a guard released early would
    // free the slot and let a later racer win it honestly.
    let admitted: Vec<_> = racers
        .into_iter()
        .filter_map(|r| r.join().expect("racer must not panic"))
        .collect();

    assert_eq!(
        admitted.len(),
        1,
        "exactly one of {} concurrent admissions may own the thread",
        RACERS
    );
}

#[test]
fn a_released_thread_admits_the_next_run() {
    // The refusal is about a LIVE turn, not about the thread id. Once the
    // guard drops, the next run is admitted normally.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let first = try_register_thread(&threads, &completions, tid)
        .admitted()
        .expect("thread is free");
    assert!(try_register_thread(&threads, &completions, tid)
        .admitted()
        .is_none());

    drop(first);
    assert!(
        try_register_thread(&threads, &completions, tid)
            .admitted()
            .is_some(),
        "a released thread must admit the next run"
    );
}

#[test]
fn a_refused_wake_still_reaches_the_live_turn() {
    // What the loser does instead of starting a second run: the prompt goes
    // into the running turn's channel. One turn reads both wakes, which is the
    // whole point of refusing rather than queueing.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (_token, mut rx, _guard) = try_register_thread(&threads, &completions, tid)
        .admitted()
        .expect("free");
    assert!(try_register_thread(&threads, &completions, tid)
        .admitted()
        .is_none());

    let injected = threads
        .lock()
        .unwrap()
        .get(&tid)
        .expect("the live turn's handle")
        .inject(test_prompt("[CHILD THREAD COMPLETED] nightly e2e"));
    assert!(injected, "the refused wake must be injectable");
    assert_eq!(
        rx.try_recv().unwrap().text,
        "[CHILD THREAD COMPLETED] nightly e2e"
    );
}

#[test]
fn different_ids_dont_interfere() {
    let threads = make_threads();
    let tid_a = Uuid::new_v4();
    let tid_b = Uuid::new_v4();
    let (token_a, _rx_a, _guard_a) = register(&threads, tid_a);
    let (token_b, _rx_b, _guard_b) = register(&threads, tid_b);
    assert!(!token_a.is_cancelled());
    assert!(!token_b.is_cancelled());
    // Dropping one doesn't cancel the other
    drop(_guard_a);
    assert!(!token_b.is_cancelled());
}

#[test]
fn cancel_thread_cancels_correct_token() {
    let threads = make_threads();
    let tid_a = Uuid::new_v4();
    let tid_b = Uuid::new_v4();
    let (token_a, _rx_a, _guard_a) = register(&threads, tid_a);
    let (token_b, _rx_b, _guard_b) = register(&threads, tid_b);

    // Cancel only thread A
    if let Some(handle) = threads.lock().unwrap().get(&tid_a) {
        handle.token.cancel();
    }
    assert!(token_a.is_cancelled());
    assert!(!token_b.is_cancelled(), "cancelling A must not cancel B");
}

#[test]
fn cancel_all_threads_cancels_all() {
    let threads = make_threads();
    let tid_a = Uuid::new_v4();
    let tid_b = Uuid::new_v4();
    let (token_a, _rx_a, _guard_a) = register(&threads, tid_a);
    let (token_b, _rx_b, _guard_b) = register(&threads, tid_b);

    for handle in threads.lock().unwrap().values() {
        handle.token.cancel();
    }
    assert!(token_a.is_cancelled());
    assert!(token_b.is_cancelled());
}

// ── cancel actor plumbing ──────────────────────────────────────────────
//
// Regression coverage for the "ResponseCanceled is missing device" gap:
// `POST /api/v1/chat/cancel` resolves the actor from the request headers,
// stamps it on `ThreadHandle.cancel_actor`, then cancels the token. The
// agentic loop's cancel arm drains the slot via `take_cancel_actor` and
// merges it into the emitted `ResponseCanceled`'s meta — so the timeline
// records WHICH device clicked Stop.
//
// These tests exercise the storage primitive in isolation (engine API not
// available here — the integration through the engine's `cancel_thread` /
// `take_cancel_actor` wrappers is identical to what's tested here, just
// behind a `LucidosEngine` method).

fn sample_device_actor(id: &str) -> thread_events::MessageOrigin {
    thread_events::MessageOrigin::Device {
        device_id: id.into(),
        label: format!("Test device {}", id),
    }
}

/// Test helper: clone the Arc<Mutex<...>> out of the handle while holding
/// the outer map lock so we can drop the map lock before manipulating the
/// inner slot — matches how the production code accesses the slot via the
/// LucidosEngine methods (which take + release the map lock before locking
/// the inner Mutex).
fn cancel_actor_arc(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
) -> Arc<std::sync::Mutex<Option<thread_events::MessageOrigin>>> {
    threads
        .lock()
        .unwrap()
        .get(&thread_id)
        .expect("thread is registered")
        .cancel_actor
        .clone()
}

#[test]
fn cancel_actor_slot_stores_and_drains() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, _rx, _guard) = register(&threads, tid);
    let slot = cancel_actor_arc(&threads, tid);

    // Stamp the actor (mirrors what `cancel_thread(tid, Some(actor))` does).
    let actor = sample_device_actor("ios-1");
    *slot.lock().unwrap() = Some(actor.clone());

    // First drain returns the stamped actor.
    let drained = slot.lock().unwrap().take();
    assert_eq!(
        drained,
        Some(actor),
        "first take must return the stamped actor"
    );

    // Second drain returns None — actors are one-shot to prevent a
    // follow-up request inheriting a stale device on the same thread.
    let drained_again = slot.lock().unwrap().take();
    assert!(
        drained_again.is_none(),
        "second take must be empty (one-shot)"
    );
}

#[test]
fn cancel_actor_slot_is_per_thread() {
    let threads = make_threads();
    let tid_a = Uuid::new_v4();
    let tid_b = Uuid::new_v4();
    let (_ta, _ra, _ga) = register(&threads, tid_a);
    let (_tb, _rb, _gb) = register(&threads, tid_b);
    let slot_a = cancel_actor_arc(&threads, tid_a);
    let slot_b = cancel_actor_arc(&threads, tid_b);

    // Stamp only on A.
    let actor = sample_device_actor("device-A");
    *slot_a.lock().unwrap() = Some(actor.clone());

    // B's slot must stay empty.
    let from_b = slot_b.lock().unwrap().take();
    assert!(
        from_b.is_none(),
        "stamping actor on A must not leak into B's slot"
    );

    let from_a = slot_a.lock().unwrap().take();
    assert_eq!(from_a, Some(actor), "A's slot still carries the actor");
}

#[test]
fn cancel_actor_slot_default_is_none() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, _rx, _guard) = register(&threads, tid);
    let slot = cancel_actor_arc(&threads, tid);

    // A freshly-registered handle has no pending cancel actor — engine-
    // internal cancels (shutdown, restart) must not inherit a phantom
    // device just because the slot exists.
    let initial = slot.lock().unwrap().take();
    assert!(initial.is_none(), "fresh handle must have empty actor slot");
}

#[test]
fn cc_spawn_scenario_original_guard_drop_does_not_cancel_cc() {
    // Simulates: original chat registers thread_id=A, spawns CC with thread_id=B.
    // When original chat completes, guard_A drops. CC's token_B must NOT be cancelled.
    let threads = make_threads();
    let original_tid = Uuid::new_v4();
    let cc_tid = Uuid::new_v4();

    let (_token_orig, _rx_orig, guard_orig) = register(&threads, original_tid);
    let (token_cc, _rx_cc, _guard_cc) = register(&threads, cc_tid);

    // Original chat completes, drops its guard
    drop(guard_orig);

    assert!(
        !token_cc.is_cancelled(),
        "CC token must survive original thread guard drop"
    );
}

#[tokio::test]
async fn idle_wait_exits_on_cancel_notify() {
    let threads = make_threads();
    let cc_tid = Uuid::new_v4();
    let (token, _injection_rx, _guard) = register(&threads, cc_tid);
    let cancel = Arc::new(tokio::sync::Notify::new());

    let token_clone = token.clone();
    let cancel_clone = cancel.clone();

    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_clone.notified() => {
                    return "cancel_notified";
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
            if token_clone.is_cancelled() {
                return "token_cancelled";
            }
        }
    });

    // Notify cancel after 50ms
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.notify_one();

    let result = tokio::time::timeout(std::time::Duration::from_millis(200), handle)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, "cancel_notified");
}

#[tokio::test]
async fn register_thread_queues_instead_of_cancelling() {
    // When a second request arrives for the same thread while the first is
    // still running, register_thread_queued must WAIT for the first to finish
    // instead of cancelling it.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    // First request registers and starts "processing"
    let (token1, _rx1, guard1) = register_queued(&threads, &completions, tid).await;
    assert!(!token1.is_cancelled());

    // Second request arrives — spawned so it can await
    let threads_c = threads.clone();
    let completions_c = completions.clone();
    let second =
        tokio::spawn(async move { register_queued(&threads_c, &completions_c, tid).await });

    // Give the second request time to start waiting
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // First request's token must NOT be cancelled while it's still processing
    assert!(
        !token1.is_cancelled(),
        "first token must not be cancelled by queued request"
    );

    // Second request should still be waiting
    assert!(
        !second.is_finished(),
        "second request must be waiting, not done"
    );

    // First request finishes — drop guard (triggers notify)
    drop(guard1);

    // Second request should now proceed
    let result = tokio::time::timeout(std::time::Duration::from_millis(200), second).await;
    assert!(
        result.is_ok(),
        "second request must unblock after first completes"
    );
    let (token2, _rx2, _guard2) = result.unwrap().unwrap();
    assert!(!token2.is_cancelled(), "second token must be fresh");
    assert!(
        !token1.is_cancelled(),
        "first token completed naturally, never cancelled"
    );
}

#[tokio::test]
async fn explicit_cancel_unblocks_queued_request() {
    // When the first request is explicitly cancelled (via cancel_thread),
    // the queued second request should proceed after the guard drops.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (token1, _rx1, guard1) = register_queued(&threads, &completions, tid).await;

    let threads_c = threads.clone();
    let completions_c = completions.clone();
    let second =
        tokio::spawn(async move { register_queued(&threads_c, &completions_c, tid).await });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!second.is_finished(), "second must be waiting");

    // Explicitly cancel the first request's token
    token1.cancel();
    // Guard drop triggers the notify — simulates the cancelled task exiting
    drop(guard1);

    let result = tokio::time::timeout(std::time::Duration::from_millis(200), second).await;
    assert!(
        result.is_ok(),
        "second request must unblock after cancel + guard drop"
    );
    let (token2, _rx2, _guard2) = result.unwrap().unwrap();
    assert!(
        !token2.is_cancelled(),
        "second token must be fresh after cancel"
    );
}

#[tokio::test]
async fn multiple_queued_requests_process_in_order() {
    // Three requests for the same thread — they should process sequentially.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));

    // First request
    let (_token1, _rx1, guard1) = register_queued(&threads, &completions, tid).await;
    order.lock().unwrap().push(1);

    // Second request (queued)
    let threads_c = threads.clone();
    let completions_c = completions.clone();
    let order_c = order.clone();
    let second = tokio::spawn(async move {
        let (t, _rx, g) = register_queued(&threads_c, &completions_c, tid).await;
        order_c.lock().unwrap().push(2);
        (t, g)
    });

    // Third request (queued behind second)
    let threads_c2 = threads.clone();
    let completions_c2 = completions.clone();
    let order_c2 = order.clone();
    let third = tokio::spawn(async move {
        let (t, _rx, g) = register_queued(&threads_c2, &completions_c2, tid).await;
        order_c2.lock().unwrap().push(3);
        (t, g)
    });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!second.is_finished(), "second must be waiting");
    assert!(!third.is_finished(), "third must be waiting");

    // First finishes
    drop(guard1);
    let result2 = tokio::time::timeout(std::time::Duration::from_millis(200), second).await;
    assert!(result2.is_ok(), "second must unblock");
    let (_token2, guard2) = result2.unwrap().unwrap();

    // Third should still be waiting (second is now active)
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!third.is_finished(), "third must wait for second");

    // Second finishes
    drop(guard2);
    let result3 = tokio::time::timeout(std::time::Duration::from_millis(200), third).await;
    assert!(result3.is_ok(), "third must unblock");

    // Verify order
    assert_eq!(
        *order.lock().unwrap(),
        vec![1, 2, 3],
        "requests must process in order"
    );
}

#[tokio::test]
async fn queued_request_for_different_thread_proceeds_immediately() {
    // Requests for different thread IDs should not block each other.
    let threads = make_threads();
    let completions = make_completions();
    let tid_a = Uuid::new_v4();
    let tid_b = Uuid::new_v4();

    let (_token_a, _rx_a, _guard_a) = register_queued(&threads, &completions, tid_a).await;

    // Request for thread B should proceed immediately (not blocked by A)
    let threads_c = threads.clone();
    let completions_c = completions.clone();
    let other =
        tokio::spawn(async move { register_queued(&threads_c, &completions_c, tid_b).await });

    let result = tokio::time::timeout(std::time::Duration::from_millis(100), other).await;
    assert!(result.is_ok(), "different thread ID must not be blocked");
}

#[tokio::test]
async fn stuck_thread_force_evicted_after_timeout() {
    // When a thread is stuck in active_threads (e.g., CC process that
    // never completes), register_thread_queued must not hang forever.
    // After the timeout, it should force-cancel and evict the stuck thread.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    // Register a "stuck" thread that never drops its guard
    let (stuck_token, _rx, _stuck_guard) = register(&threads, tid);
    assert!(!stuck_token.is_cancelled());

    // A follow-up request with a short timeout (200ms for test speed)
    let threads_c = threads.clone();
    let completions_c = completions.clone();
    let second = tokio::spawn(async move {
        register_queued_with_timeout(
            &threads_c,
            &completions_c,
            tid,
            std::time::Duration::from_millis(200),
        )
        .await
    });

    // The follow-up must complete within a reasonable time (timeout + margin)
    let result = tokio::time::timeout(std::time::Duration::from_millis(500), second).await;
    assert!(
        result.is_ok(),
        "follow-up must not hang forever — timeout should evict stuck thread"
    );
    let (token2, _rx2, _guard2) = result.unwrap().unwrap();
    assert!(!token2.is_cancelled(), "new token must be fresh");
    // The stuck thread's token must have been cancelled
    assert!(
        stuck_token.is_cancelled(),
        "stuck thread token must be force-cancelled"
    );
}

#[tokio::test]
async fn the_stuck_turn_eviction_hands_the_slot_straight_over() {
    // The one sanctioned way past a refusal, and the only place a handle is
    // replaced rather than added. Freeing the slot and taking it are one step,
    // so nothing can slip in between. The evicted turn keeps unwinding, and
    // its guard must not then remove the replacement.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (stuck_token, _stuck_rx, stuck_guard) = try_register_thread(&threads, &completions, tid)
        .admitted()
        .expect("thread is free");
    let stuck_generation = stuck_guard.generation();

    let (fresh_token, _fresh_rx, _fresh_guard) =
        evict_and_register(&threads, &completions, tid, stuck_generation)
            .expect("the wedged turn is still the holder");
    assert!(
        stuck_token.is_cancelled(),
        "the evicted turn must be told to stop"
    );
    assert!(!fresh_token.is_cancelled(), "the replacement runs");
    assert_eq!(threads.lock().unwrap().len(), 1);

    drop(stuck_guard);
    assert!(
        threads.lock().unwrap().contains_key(&tid),
        "the evicted guard must not remove the replacement (generation mismatch)"
    );
}

#[tokio::test]
async fn a_second_timed_out_waiter_cannot_evict_the_first_ones_replacement() {
    // Several follow-ups queue behind one wedged turn, so their 60 s budgets
    // expire together. The first evicts and installs a replacement, and that
    // replacement is live work. An unconditional eviction let the next waiter
    // remove it, cancel it, and install a third handle. That is the very
    // two-runs-on-one-thread state the single-flight guard exists to prevent.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (_stuck_token, _stuck_rx, stuck_guard) = try_register_thread(&threads, &completions, tid)
        .admitted()
        .expect("thread is free");
    // Both waiters observed the wedged turn before either timed out.
    let stuck_generation = stuck_guard.generation();

    let (first_token, _first_rx, _first_guard) =
        evict_and_register(&threads, &completions, tid, stuck_generation)
            .expect("the first waiter evicts the wedged turn");

    assert!(
        evict_and_register(&threads, &completions, tid, stuck_generation).is_none(),
        "the second waiter must be refused: the slot is a live replacement now"
    );
    assert!(
        !first_token.is_cancelled(),
        "the first waiter's turn must keep running"
    );
    assert_eq!(threads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn eviction_is_refused_once_the_wedged_turn_releases_on_its_own() {
    // The other way the expected generation goes stale: the turn finished
    // between the budget expiring and the eviction. There is nothing to evict,
    // and the caller goes back to ordinary admission.
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (_token, _rx, guard) = try_register_thread(&threads, &completions, tid)
        .admitted()
        .expect("thread is free");
    let generation = guard.generation();
    drop(guard);

    assert!(
        evict_and_register(&threads, &completions, tid, generation).is_none(),
        "an empty slot is not something to evict"
    );
    assert!(
        threads.lock().unwrap().is_empty(),
        "a refused eviction must install nothing"
    );
}

#[tokio::test]
async fn idle_wait_exits_on_token_cancel() {
    let threads = make_threads();
    let cc_tid = Uuid::new_v4();
    let (token, _injection_rx, _guard) = register(&threads, cc_tid);
    let cancel = Arc::new(tokio::sync::Notify::new());

    let token_clone = token.clone();
    let cancel_clone = cancel.clone();

    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_clone.notified() => {
                    return "cancel_notified";
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
            if token_clone.is_cancelled() {
                return "token_cancelled";
            }
        }
    });

    // Cancel the token after 50ms (simulates cancel_thread or cancel_all_threads)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    token.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_millis(200), handle)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, "token_cancelled");
}
