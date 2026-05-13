use super::*;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn unique_workspace(label: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "lucidos_authmod_migration_{}_{}",
        label,
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn migration_renames_legacy_auth_modules_when_new_absent() {
    let ws = unique_workspace("rename");
    let legacy = ws.join("data/artifacts/auth-modules");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("foo.wasm"), b"\0asm").unwrap();

    super::migrate_legacy_auth_modules_dir(&ws);

    let new_dir = ws.join("data/auth-modules");
    assert!(new_dir.is_dir(), "new dir should exist after migration");
    assert!(new_dir.join("foo.wasm").is_file());
    assert!(!legacy.exists(), "legacy dir should be gone after rename");

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn migration_no_op_when_legacy_absent() {
    let ws = unique_workspace("none");
    super::migrate_legacy_auth_modules_dir(&ws);
    assert!(!ws.join("data/auth-modules").exists());
    assert!(!ws.join("data/artifacts/auth-modules").exists());
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn migration_does_not_clobber_when_both_exist() {
    let ws = unique_workspace("both");
    let legacy = ws.join("data/artifacts/auth-modules");
    let new_dir = ws.join("data/auth-modules");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("legacy.wasm"), b"legacy").unwrap();
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::write(new_dir.join("new.wasm"), b"new").unwrap();

    super::migrate_legacy_auth_modules_dir(&ws);

    assert!(
        legacy.join("legacy.wasm").is_file(),
        "legacy must be untouched"
    );
    assert!(new_dir.join("new.wasm").is_file(), "new must be untouched");
    assert!(!new_dir.join("legacy.wasm").exists(), "must not merge");

    let _ = std::fs::remove_dir_all(&ws);
}

/// Create a standalone active_threads map for testing thread registration.
fn make_threads() -> Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>> {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// Create a standalone completion notifiers map for testing.
fn make_completions() -> Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>> {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// Replicate register_thread logic for standalone testing (sync, no queuing).
fn register(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
) -> (
    CancellationToken,
    mpsc::UnboundedReceiver<InjectedPrompt>,
    ThreadGuard,
) {
    let token = CancellationToken::new();
    let (injection_tx, injection_rx) = mpsc::unbounded_channel();
    let gen = THREAD_GENERATION.fetch_add(1, Ordering::Relaxed);
    let mut guard_map = threads.lock().unwrap();
    guard_map.insert(
        thread_id,
        ThreadHandle {
            token: token.clone(),
            injection_tx,
            generation: gen,
        },
    );
    let guard = ThreadGuard {
        active_threads: threads.clone(),
        thread_id,
        completion_notify: Arc::new(std::sync::Mutex::new(HashMap::new())),
        generation: gen,
    };
    (token, injection_rx, guard)
}

/// Replicate register_thread_queued logic: waits for existing thread to
/// finish before registering. Force-evicts after `timeout`.
async fn register_queued_with_timeout(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    thread_id: Uuid,
    timeout: std::time::Duration,
) -> (
    CancellationToken,
    mpsc::UnboundedReceiver<InjectedPrompt>,
    ThreadGuard,
) {
    let wait_result = tokio::time::timeout(timeout, async {
        loop {
            let n = {
                let guard_map = threads.lock().unwrap();
                if guard_map.contains_key(&thread_id) {
                    let mut comps = completions.lock().unwrap();
                    comps
                        .entry(thread_id)
                        .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
                        .clone()
                } else {
                    return;
                }
            };
            tokio::select! {
                _ = n.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }
        }
    })
    .await;

    if wait_result.is_err() {
        if let Some(handle) = threads.lock().unwrap().remove(&thread_id) {
            handle.token.cancel();
        }
        if let Some(n) = completions.lock().unwrap().remove(&thread_id) {
            n.notify_waiters();
        }
    }

    let token = CancellationToken::new();
    let (injection_tx, injection_rx) = mpsc::unbounded_channel();
    let gen = THREAD_GENERATION.fetch_add(1, Ordering::Relaxed);
    let mut guard_map = threads.lock().unwrap();
    guard_map.insert(
        thread_id,
        ThreadHandle {
            token: token.clone(),
            injection_tx,
            generation: gen,
        },
    );
    let guard = ThreadGuard {
        active_threads: threads.clone(),
        thread_id,
        completion_notify: completions.clone(),
        generation: gen,
    };
    (token, injection_rx, guard)
}

/// Convenience wrapper with the default 60s timeout.
async fn register_queued(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    thread_id: Uuid,
) -> (
    CancellationToken,
    mpsc::UnboundedReceiver<InjectedPrompt>,
    ThreadGuard,
) {
    register_queued_with_timeout(
        threads,
        completions,
        thread_id,
        std::time::Duration::from_secs(60),
    )
    .await
}

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
    // This ensures CC sessions aren't killed when guards drop
    assert!(!token.is_cancelled(), "guard drop must not cancel token");
}

#[test]
fn reregister_same_id_replaces_token_without_cancel() {
    // register_thread (sync) replaces the token in the map but does NOT
    // cancel the old one. Cancellation is only done by explicit cancel_thread.
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (old_token, _old_rx, _old_guard) = register(&threads, tid);
    let (new_token, _new_rx, _new_guard) = register(&threads, tid);
    assert!(
        !old_token.is_cancelled(),
        "old token must NOT be cancelled on re-register"
    );
    assert!(!new_token.is_cancelled(), "new token must not be cancelled");
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
async fn old_guard_drop_does_not_remove_new_registration() {
    // After force-eviction, the old ThreadGuard still exists. When it
    // drops, it must NOT remove the new registration (different generation).
    let threads = make_threads();
    let tid = Uuid::new_v4();

    // Register old thread
    let (_old_token, _old_rx, old_guard) = register(&threads, tid);
    assert!(threads.lock().unwrap().contains_key(&tid));

    // Force-evict: remove old handle, register new one (simulates timeout path)
    threads.lock().unwrap().remove(&tid);
    let (_new_token, _new_rx, _new_guard) = register(&threads, tid);
    assert!(threads.lock().unwrap().contains_key(&tid));

    // Drop the old guard — must NOT remove the new registration
    drop(old_guard);
    assert!(
        threads.lock().unwrap().contains_key(&tid),
        "old guard drop must not remove new registration (different generation)"
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

// --- Injection channel tests ---

#[test]
fn inject_prompt_delivers_to_active_thread() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    // Inject a message
    let injected = {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "fix the bug".into(),
                event_id: None,
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .is_ok()
    };
    assert!(injected, "inject_prompt must succeed for active thread");

    // Receiver should have the message
    let msg = rx.try_recv().unwrap();
    assert_eq!(msg.text, "fix the bug");
}

#[test]
fn inject_prompt_fails_for_unknown_thread() {
    let threads = make_threads();
    let tid = Uuid::new_v4();

    // No thread registered — inject should fail
    let result = {
        let map = threads.lock().unwrap();
        map.get(&tid).map(|h| {
            h.injection_tx
                .send(InjectedPrompt {
                    text: "msg".into(),
                    event_id: None,
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                    kind: crate::engine::InjectedPromptKind::UserText,
                })
                .is_ok()
        })
    };
    assert!(result.is_none(), "inject must fail for unknown thread");
}

#[test]
fn inject_prompt_fails_after_guard_drop() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, _rx, guard) = register(&threads, tid);

    // Drop the guard — thread is deregistered
    drop(guard);

    let result = {
        let map = threads.lock().unwrap();
        map.get(&tid).map(|h| {
            h.injection_tx
                .send(InjectedPrompt {
                    text: "msg".into(),
                    event_id: None,
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                    kind: crate::engine::InjectedPromptKind::UserText,
                })
                .is_ok()
        })
    };
    assert!(
        result.is_none(),
        "inject must fail after thread deregistered"
    );
}

#[test]
fn inject_multiple_prompts_drains_in_order() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    // Send multiple injections
    {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "first".into(),
                event_id: None,
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "second".into(),
                event_id: None,
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "third".into(),
                event_id: None,
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
    }

    // Drain all — should come in order
    let mut texts = Vec::new();
    while let Ok(prompt) = rx.try_recv() {
        texts.push(prompt.text);
    }
    assert_eq!(texts, vec!["first", "second", "third"]);
}

#[test]
fn inject_prompt_does_not_cancel_thread() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (token, _rx, _guard) = register(&threads, tid);

    {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "correction".into(),
                event_id: None,
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
    }

    assert!(
        !token.is_cancelled(),
        "injecting must not cancel the thread"
    );
    assert!(
        threads.lock().unwrap().contains_key(&tid),
        "thread must remain active"
    );
}

#[test]
fn inject_prompt_preserves_event_id() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let eid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "fix".into(),
                event_id: Some(eid),
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
    }

    let prompt = rx.try_recv().unwrap();
    assert_eq!(prompt.text, "fix");
    assert_eq!(prompt.event_id, Some(eid));
}

#[test]
fn inject_prompt_preserves_system_source() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "[Child thread completed] some task".into(),
                event_id: None,
                mode: thread_events::ActorMode::Agent,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
    }

    let prompt = rx.try_recv().unwrap();
    assert_eq!(prompt.mode, thread_events::ActorMode::Agent);
    assert!(prompt.text.contains("Child thread completed"));
}

// --- Orphaned injection tests ---

#[test]
fn orphaned_injection_is_recovered_by_drain() {
    // Bug: when a follow-up arrives via inject_prompt() after the agentic
    // loop's last try_recv but before the ThreadGuard drops, the message
    // sits in injection_rx and is silently lost when the function returns.
    //
    // This test verifies drain_orphaned_injections() recovers the message.
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, _guard) = register(&threads, tid);

    // Simulate: agentic loop has finished (ResponseGenerated emitted).
    // A user's follow-up arrives via inject_prompt while thread is still
    // active (guard alive, thread in active_threads).
    {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "follow-up message".to_string(),
                event_id: Some(Uuid::new_v4()),
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
    }

    let orphans = LucidosEngine::drain_orphaned_injections(&mut injection_rx);
    assert_eq!(
        orphans.len(),
        1,
        "orphaned injection must be recovered, not silently lost"
    );
    assert_eq!(orphans[0].text, "follow-up message");
}

#[test]
fn drain_orphaned_injections_returns_empty_when_no_orphans() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, _guard) = register(&threads, tid);

    let orphans = LucidosEngine::drain_orphaned_injections(&mut injection_rx);
    assert!(orphans.is_empty(), "no orphans when nothing was injected");
}

#[test]
fn drain_orphaned_injections_recovers_multiple_in_order() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, _guard) = register(&threads, tid);

    {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "first follow-up".to_string(),
                event_id: None,
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
        handle
            .injection_tx
            .send(InjectedPrompt {
                text: "second follow-up".to_string(),
                event_id: None,
                mode: thread_events::ActorMode::Agent,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .unwrap();
    }

    let orphans = LucidosEngine::drain_orphaned_injections(&mut injection_rx);
    assert_eq!(orphans.len(), 2);
    assert_eq!(orphans[0].text, "first follow-up");
    assert_eq!(orphans[1].text, "second follow-up");
    // Verify mode is preserved
    assert_eq!(orphans[0].mode, thread_events::ActorMode::Human);
    assert_eq!(orphans[1].mode, thread_events::ActorMode::Agent);
}

// --- CC exit / follow-up tests ---

/// With no idle waiting loop, when CC exits the ThreadGuard drops immediately,
/// allowing follow-up requests to register without any cancel hack.
#[tokio::test]
async fn cc_exit_drops_guard_allows_follow_up() {
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    // 1. Simulate CC holding the thread
    {
        let (_token, _rx, _guard) = register(&threads, tid);
        assert!(threads.lock().unwrap().contains_key(&tid));
        // CC exits — guard drops here
    }

    // 2. Thread should be free immediately
    assert!(!threads.lock().unwrap().contains_key(&tid));

    // 3. Follow-up can register without any cancel hack
    let (new_token, _new_rx, _new_guard) = register_queued(&threads, &completions, tid).await;
    assert!(!new_token.is_cancelled());
}

#[test]
fn partition_chat_thread_ids_excludes_idle_cc_session() {
    use std::collections::HashSet;
    let chat_only = Uuid::new_v4();
    let cc_in_flight = Uuid::new_v4();
    let cc_idle = Uuid::new_v4();
    let processing = vec![chat_only, cc_in_flight, cc_idle];
    let all_cc: HashSet<Uuid> = [cc_in_flight, cc_idle].into_iter().collect();

    let chat = partition_chat_thread_ids(&processing, &all_cc);

    assert_eq!(chat, vec![chat_only]);
}

#[test]
fn partition_chat_thread_ids_keeps_chat_threads() {
    use std::collections::HashSet;
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let processing = vec![a, b];
    let all_cc: HashSet<Uuid> = HashSet::new();

    let chat = partition_chat_thread_ids(&processing, &all_cc);

    assert_eq!(chat.len(), 2);
    assert!(chat.contains(&a));
    assert!(chat.contains(&b));
}
