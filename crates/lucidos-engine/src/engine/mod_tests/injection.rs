use super::common::*;
use super::*;
use uuid::Uuid;

fn test_prompt(text: &str) -> InjectedPrompt {
    InjectedPrompt {
        text: text.into(),
        event_id: None,
        mode: thread_events::ActorMode::Human,
        spawning_event_id: None,
        images: None,
        origin: None,
        kind: crate::engine::InjectedPromptKind::UserText,
    }
}

/// A blocking tool (`bash_output(wait_secs=120)` is the one that really
/// blocks) parks on `injection_notify` so a follow-up doesn't sit unread
/// for two minutes — the agentic loop only `try_recv`s injections BETWEEN
/// iterations, so nothing else would wake it.
#[tokio::test]
async fn inject_wakes_a_tool_blocked_on_this_thread() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    let notify = {
        let map = threads.lock().unwrap();
        map.get(&tid).unwrap().injection_notify.clone()
    };

    // Park first, exactly as the drain does, then inject.
    let waiter = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), notify.notified())
            .await
            .is_ok()
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let injected = {
        let map = threads.lock().unwrap();
        map.get(&tid)
            .unwrap()
            .inject(test_prompt("also open the site"))
    };
    assert!(injected);
    assert!(waiter.await.unwrap(), "a parked tool must wake on inject");
    // The prompt itself still reaches the loop — waking is in addition to
    // delivery, not instead of it.
    assert_eq!(rx.try_recv().unwrap().text, "also open the site");
}

/// The wide window a bare notification misses: the user types while the LLM
/// call is in flight, so the prompt is already queued by the time the tool
/// call it produced starts blocking. No waiter existed when `notify_waiters`
/// fired, and it leaves no permit — the unread count is the only evidence,
/// and a blocking tool must refuse to block on it.
#[test]
fn pending_count_survives_an_injection_with_nobody_listening() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    let pending = {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle.inject(test_prompt("stop, the site is wrong"));
        handle.pending_injections.clone()
    };
    assert_eq!(
        pending.load(std::sync::atomic::Ordering::Acquire),
        1,
        "an injection nobody was waiting for must still be visible as unread"
    );

    // The loop drains it on its next iteration and reports what it took.
    let drained = {
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        n
    };
    threads
        .lock()
        .unwrap()
        .get(&tid)
        .unwrap()
        .injections_drained(drained);
    assert_eq!(
        pending.load(std::sync::atomic::Ordering::Acquire),
        0,
        "a drained injection is no longer unread — later waits must block normally"
    );
}

/// A failed send must leave no reservation behind. `inject` counts before
/// sending (a drain can otherwise report a prompt consumed before a
/// post-send increment lands, stranding a phantom unread that would stop
/// every later wait from blocking) — so the failure path has to give the
/// reservation back, or a dead thread's handle poisons nothing but is still
/// a lie about unread work.
#[test]
fn failed_inject_rolls_back_its_reservation() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, rx, _guard) = register(&threads, tid);
    drop(rx); // the turn ended — the receiver is gone

    let map = threads.lock().unwrap();
    let handle = map.get(&tid).unwrap();
    assert!(
        !handle.inject(test_prompt("too late")),
        "inject must report failure once the receiver is gone"
    );
    assert_eq!(
        handle
            .pending_injections
            .load(std::sync::atomic::Ordering::Acquire),
        0,
        "a prompt that was never delivered must not count as unread"
    );
}

/// A drain reported by a force-evicted turn must not touch the turn that
/// replaced it. `register_thread_queued` evicts a turn stuck for 60 s and
/// registers a new handle under the same thread_id while the old loop is
/// still unwinding; if the old loop's drain decremented by thread_id alone
/// it would erase the NEW turn's unread follow-up, and `bash_output` would
/// block straight through the user's message. Same generation guard
/// `ThreadGuard::drop` uses.
#[test]
fn a_stale_generations_drain_cannot_erase_the_new_turns_unread_count() {
    let engine_threads = make_threads();
    let tid = Uuid::new_v4();

    // Turn 1 registers, then is force-evicted and replaced by turn 2 —
    // re-registering overwrites the map entry with a fresh generation.
    let (_t1, _rx1, guard1) = register(&engine_threads, tid);
    let stale_generation = guard1.generation();
    let (_t2, _rx2, guard2) = register(&engine_threads, tid);
    assert_ne!(
        stale_generation,
        guard2.generation(),
        "a re-registration must take a fresh generation"
    );
    // Turn 1's guard drops late (its loop is still unwinding). Its own
    // generation check already stops it removing turn 2's registration.
    drop(guard1);
    assert!(
        engine_threads.lock().unwrap().contains_key(&tid),
        "the stale guard must leave the live registration in place"
    );

    // The user types into turn 2.
    let pending = {
        let map = engine_threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle.inject(test_prompt("wait, stop"));
        handle.pending_injections.clone()
    };

    // Turn 1's loop finally drains its own (old) receiver and reports it.
    let stale = {
        let map = engine_threads.lock().unwrap();
        map.get(&tid)
            .filter(|h| h.generation == stale_generation)
            .map(|h| h.injections_drained(1))
    };
    assert!(
        stale.is_none(),
        "the stale generation must not resolve to the live handle"
    );
    assert_eq!(
        pending.load(std::sync::atomic::Ordering::Acquire),
        1,
        "turn 2's follow-up is still unread — a blocking tool must not sit on it"
    );
}

/// The counter tracks the channel and must never wrap: an over-report (a
/// drain site double-counting, or a prompt filtered out before delivery)
/// saturates at zero instead of underflowing to usize::MAX, which would
/// make every subsequent wait return instantly forever.
#[test]
fn injections_drained_saturates_instead_of_wrapping() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, _rx, _guard) = register(&threads, tid);

    let map = threads.lock().unwrap();
    let handle = map.get(&tid).unwrap();
    handle.inject(test_prompt("one"));
    handle.injections_drained(5);
    assert_eq!(
        handle
            .pending_injections
            .load(std::sync::atomic::Ordering::Acquire),
        0
    );
}

/// `notify_waiters`, not `notify_one`: a stored permit would make the very
/// next `wait_secs` block return instantly for a message the loop has
/// already consumed — reintroducing the polling storm one drain later.
#[tokio::test]
async fn inject_leaves_no_permit_for_a_later_wait() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    let notify = {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        // Nobody is parked yet — this notification has no waiter to wake.
        handle.inject(test_prompt("early"));
        handle.injection_notify.clone()
    };
    assert_eq!(rx.try_recv().unwrap().text, "early");

    let woke = tokio::time::timeout(std::time::Duration::from_millis(200), notify.notified())
        .await
        .is_ok();
    assert!(
        !woke,
        "a consumed injection must not leave a permit that cuts the next wait short"
    );
}

#[test]
fn injection_channel_delivers_to_active_thread() {
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
    assert!(injected, "injection_tx send must succeed for active thread");

    // Receiver should have the message
    let msg = rx.try_recv().unwrap();
    assert_eq!(msg.text, "fix the bug");
}

#[test]
fn injection_channel_unavailable_for_unknown_thread() {
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
fn injection_channel_unavailable_after_guard_drop() {
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
fn injection_channel_send_does_not_cancel_thread() {
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
fn injection_channel_preserves_event_id() {
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
fn injection_channel_preserves_actor_mode() {
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
    // Bug: when a follow-up arrives on injection_tx after the agentic
    // loop's last try_recv but before the ThreadGuard drops, the message
    // sits in injection_rx and is silently lost when the function returns.
    //
    // This test verifies drain_orphaned_injections() recovers the message.
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, _guard) = register(&threads, tid);

    // Simulate: agentic loop has finished (ResponseGenerated emitted).
    // A user's follow-up arrives on injection_tx while thread is still
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

// --- Finalize-window race: teardown must remove-then-drain ---

#[test]
fn finalize_turn_recovers_followup_that_raced_teardown() {
    use std::sync::Barrier;
    // Deterministic reproduction of the finalize-window race. The fast-path
    // follow-up send takes the active_threads lock, looks up the handle, and
    // sends WHILE HOLDING that lock. Teardown removes the handle under the
    // SAME lock. Here we pin the interleaving so the send is acknowledged
    // *during* teardown — exactly the window that used to drop the message:
    //
    //   1. a "sender" thread grabs the active_threads lock (an in-flight send),
    //   2. the main thread calls finalize → blocks at drop(guard) on the lock,
    //   3. the sender sends the follow-up (acknowledged) and releases the lock,
    //   4. finalize's removal proceeds, THEN it drains.
    //
    // remove-then-drain recovers the message because the drain runs after the
    // lock was reacquired (post-send). The old drain-then-remove order drained
    // BEFORE taking the lock — at step 3-time the buffer was still empty — so
    // the acknowledged message was lost. This test fails under that order.
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, guard) = register(&threads, tid);

    let lock_ready = Arc::new(Barrier::new(2));
    let lock_ready_sender = lock_ready.clone();
    let threads_for_send = threads.clone();

    let sender = std::thread::spawn(move || {
        // Hold the active_threads lock — models an in-flight fast-path send.
        let map = threads_for_send.lock().unwrap();
        lock_ready_sender.wait();
        // Let the main thread reach drop(guard) and block on the lock.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let ok = map
            .get(&tid)
            .expect("handle still present mid-teardown")
            .injection_tx
            .send(InjectedPrompt {
                text: "raced follow-up".to_string(),
                event_id: Some(Uuid::new_v4()),
                mode: thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
            .is_ok();
        assert!(ok, "send under the lock must be acknowledged");
        drop(map); // release — finalize's removal can now proceed
    });

    lock_ready.wait();
    // Blocks at drop(guard) until the sender releases the lock above.
    let orphans = LucidosEngine::finalize_turn_and_drain_injections(guard, &mut injection_rx);
    sender.join().unwrap();

    assert_eq!(
        orphans.len(),
        1,
        "follow-up acknowledged during teardown must be recovered, not dropped"
    );
    assert_eq!(orphans[0].text, "raced follow-up");
}

#[test]
fn finalize_turn_closes_the_inject_gate() {
    // After teardown the handle is gone, so a LATER fast-path send can no
    // longer be acknowledged into a channel nobody drains — it fails, which
    // routes the caller to the slow path (a fresh turn) instead of vanishing.
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, guard) = register(&threads, tid);

    let orphans = LucidosEngine::finalize_turn_and_drain_injections(guard, &mut injection_rx);
    assert!(orphans.is_empty(), "no orphans when nothing raced");

    // Handle removed — the inject gate is closed.
    assert!(
        !threads.lock().unwrap().contains_key(&tid),
        "finalize must remove the thread from active_threads"
    );
    let post_teardown_send = {
        let map = threads.lock().unwrap();
        map.get(&tid).map(|h| {
            h.injection_tx
                .send(InjectedPrompt {
                    text: "too late".into(),
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
        post_teardown_send.is_none(),
        "post-teardown inject must fail so the caller takes the slow path"
    );
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

// --- urgent follow-up redirect (the Lucidos Agent lane) --------------------

/// The redirect flag starts clear, so an ordinary Stop keeps meaning
/// `UserStop`. Only `cancel_thread_for_followup` sets it.
#[test]
fn redirect_followup_starts_clear() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, _rx, _guard) = register(&threads, tid);

    let handle_flag = {
        let map = threads.lock().unwrap();
        map.get(&tid).unwrap().redirect_followup.clone()
    };
    assert!(
        !handle_flag.load(Ordering::Acquire),
        "a fresh turn must not look like a redirect: that would relabel a real Stop"
    );
}

/// Drained on read, exactly like `cancel_actor`. A flag left set would relabel
/// the NEXT turn's Stop on the same thread as a redirect, which would then be
/// withheld from the parent as a non-terminal outcome and leave the parent
/// waiting on a child that had stopped.
#[test]
fn redirect_followup_is_drained_on_read() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, _rx, _guard) = register(&threads, tid);

    let flag = {
        let map = threads.lock().unwrap();
        map.get(&tid).unwrap().redirect_followup.clone()
    };
    flag.store(true, Ordering::Release);

    assert!(
        flag.swap(false, Ordering::AcqRel),
        "the first read sees the redirect"
    );
    assert!(
        !flag.swap(false, Ordering::AcqRel),
        "the second read must not: a stale flag would relabel the next turn"
    );
}
