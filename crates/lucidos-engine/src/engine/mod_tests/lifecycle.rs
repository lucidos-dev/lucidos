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
    assert!(!token.is_cancelled());
}

#[tokio::test]
async fn a_turn_keeps_the_computer_awake_until_its_guard_drops() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let label = tid.to_string();
    let (_token, _injection_rx, guard) = register(&threads, tid);
    assert!(crate::core::keep_awake::is_held(&label));

    let turn = tokio::spawn(async move {
        let _guard = guard;
        std::future::pending::<()>().await;
    });
    turn.abort();
    let _ = turn.await;
    assert!(
        !crate::core::keep_awake::is_held(&label),
        "a cancelled turn leaked its hold"
    );
}

#[test]
fn guard_drop_does_not_cancel_token() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (token, _injection_rx, guard) = register(&threads, tid);
    drop(guard);
    assert!(!token.is_cancelled(), "guard drop must not cancel token");
}

#[test]
fn two_wakes_for_one_child_completion_admit_one_run() {
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

fn sample_device_actor(id: &str) -> thread_events::MessageOrigin {
    thread_events::MessageOrigin::Device {
        device_id: id.into(),
    }
}

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

    let actor = sample_device_actor("ios-1");
    *slot.lock().unwrap() = Some(actor.clone());

    let drained = slot.lock().unwrap().take();
    assert_eq!(
        drained,
        Some(actor),
        "first take must return the stamped actor"
    );

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

    let actor = sample_device_actor("device-A");
    *slot_a.lock().unwrap() = Some(actor.clone());

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

    let initial = slot.lock().unwrap().take();
    assert!(initial.is_none(), "fresh handle must have empty actor slot");
}

#[test]
fn cc_spawn_scenario_original_guard_drop_does_not_cancel_cc() {
    let threads = make_threads();
    let original_tid = Uuid::new_v4();
    let cc_tid = Uuid::new_v4();

    let (_token_orig, _rx_orig, guard_orig) = register(&threads, original_tid);
    let (token_cc, _rx_cc, _guard_cc) = register(&threads, cc_tid);

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
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (token1, _rx1, guard1) = register_queued(&threads, &completions, tid).await;
    assert!(!token1.is_cancelled());

    let threads_c = threads.clone();
    let completions_c = completions.clone();
    let second =
        tokio::spawn(async move { register_queued(&threads_c, &completions_c, tid).await });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    assert!(
        !token1.is_cancelled(),
        "first token must not be cancelled by queued request"
    );

    assert!(
        !second.is_finished(),
        "second request must be waiting, not done"
    );

    drop(guard1);

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

    token1.cancel();
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
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));

    let (_token1, _rx1, guard1) = register_queued(&threads, &completions, tid).await;
    order.lock().unwrap().push(1);

    let threads_c = threads.clone();
    let completions_c = completions.clone();
    let order_c = order.clone();
    let second = tokio::spawn(async move {
        let (t, _rx, g) = register_queued(&threads_c, &completions_c, tid).await;
        order_c.lock().unwrap().push(2);
        (t, g)
    });

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

    drop(guard1);
    let result2 = tokio::time::timeout(std::time::Duration::from_millis(200), second).await;
    assert!(result2.is_ok(), "second must unblock");
    let (_token2, guard2) = result2.unwrap().unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!third.is_finished(), "third must wait for second");

    drop(guard2);
    let result3 = tokio::time::timeout(std::time::Duration::from_millis(200), third).await;
    assert!(result3.is_ok(), "third must unblock");

    assert_eq!(
        *order.lock().unwrap(),
        vec![1, 2, 3],
        "requests must process in order"
    );
}

#[tokio::test]
async fn queued_request_for_different_thread_proceeds_immediately() {
    let threads = make_threads();
    let completions = make_completions();
    let tid_a = Uuid::new_v4();
    let tid_b = Uuid::new_v4();

    let (_token_a, _rx_a, _guard_a) = register_queued(&threads, &completions, tid_a).await;

    let threads_c = threads.clone();
    let completions_c = completions.clone();
    let other =
        tokio::spawn(async move { register_queued(&threads_c, &completions_c, tid_b).await });

    let result = tokio::time::timeout(std::time::Duration::from_millis(100), other).await;
    assert!(result.is_ok(), "different thread ID must not be blocked");
}

#[tokio::test]
async fn stuck_thread_force_evicted_after_timeout() {
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (stuck_token, _rx, _stuck_guard) = register(&threads, tid);
    assert!(!stuck_token.is_cancelled());

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

    let result = tokio::time::timeout(std::time::Duration::from_millis(500), second).await;
    assert!(
        result.is_ok(),
        "follow-up must not hang forever — timeout should evict stuck thread"
    );
    let (token2, _rx2, _guard2) = result.unwrap().unwrap();
    assert!(!token2.is_cancelled(), "new token must be fresh");
    assert!(
        stuck_token.is_cancelled(),
        "stuck thread token must be force-cancelled"
    );
}

#[tokio::test]
async fn the_stuck_turn_eviction_hands_the_slot_straight_over() {
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
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (_stuck_token, _stuck_rx, stuck_guard) = try_register_thread(&threads, &completions, tid)
        .admitted()
        .expect("thread is free");
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

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    token.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_millis(200), handle)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, "token_cancelled");
}
