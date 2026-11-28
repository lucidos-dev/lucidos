use super::common::*;
use super::*;
use uuid::Uuid;

#[tokio::test]
async fn inject_wakes_a_tool_blocked_on_this_thread() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    let notify = {
        let map = threads.lock().unwrap();
        map.get(&tid).unwrap().injection_notify.clone()
    };

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
    assert_eq!(rx.try_recv().unwrap().text, "also open the site");
}

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

#[test]
fn failed_inject_rolls_back_its_reservation() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, rx, _guard) = register(&threads, tid);
    drop(rx);

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

#[test]
fn a_stale_generations_drain_cannot_erase_the_new_turns_unread_count() {
    let engine_threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    let (_t1, _rx1, guard1) = register(&engine_threads, tid);
    let stale_generation = guard1.generation();
    let (_t2, _rx2, guard2) =
        evict_and_register(&engine_threads, &completions, tid, stale_generation)
            .expect("turn 1 is still the holder");
    assert_ne!(
        stale_generation,
        guard2.generation(),
        "a re-registration must take a fresh generation"
    );
    drop(guard1);
    assert!(
        engine_threads.lock().unwrap().contains_key(&tid),
        "the stale guard must leave the live registration in place"
    );

    let pending = {
        let map = engine_threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
        handle.inject(test_prompt("wait, stop"));
        handle.pending_injections.clone()
    };

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

#[tokio::test]
async fn inject_leaves_no_permit_for_a_later_wait() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut rx, _guard) = register(&threads, tid);

    let notify = {
        let map = threads.lock().unwrap();
        let handle = map.get(&tid).unwrap();
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

    let msg = rx.try_recv().unwrap();
    assert_eq!(msg.text, "fix the bug");
}

#[test]
fn injection_channel_unavailable_for_unknown_thread() {
    let threads = make_threads();
    let tid = Uuid::new_v4();

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

#[test]
fn orphaned_injection_is_recovered_by_drain() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, _guard) = register(&threads, tid);

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
    assert_eq!(orphans[0].mode, thread_events::ActorMode::Human);
    assert_eq!(orphans[1].mode, thread_events::ActorMode::Agent);
}

#[test]
fn finalize_turn_recovers_followup_that_raced_teardown() {
    use std::sync::Barrier;
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, guard) = register(&threads, tid);

    let lock_ready = Arc::new(Barrier::new(2));
    let lock_ready_sender = lock_ready.clone();
    let threads_for_send = threads.clone();

    let sender = std::thread::spawn(move || {
        let map = threads_for_send.lock().unwrap();
        lock_ready_sender.wait();
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
        drop(map);
    });

    lock_ready.wait();
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
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, mut injection_rx, guard) = register(&threads, tid);

    let orphans = LucidosEngine::finalize_turn_and_drain_injections(guard, &mut injection_rx);
    assert!(orphans.is_empty(), "no orphans when nothing raced");

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

#[tokio::test]
async fn cc_exit_drops_guard_allows_follow_up() {
    let threads = make_threads();
    let completions = make_completions();
    let tid = Uuid::new_v4();

    {
        let (_token, _rx, _guard) = register(&threads, tid);
        assert!(threads.lock().unwrap().contains_key(&tid));
    }

    assert!(!threads.lock().unwrap().contains_key(&tid));

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

#[test]
fn cancel_cause_starts_empty() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, _rx, _guard) = register(&threads, tid);

    let cause = {
        let map = threads.lock().unwrap();
        map.get(&tid).unwrap().cancel_cause.clone()
    };
    assert!(
        cause.lock().unwrap().is_none(),
        "a fresh turn must not look like a redirect: that would relabel a real Stop"
    );
}

#[test]
fn cancel_cause_is_drained_on_read() {
    let threads = make_threads();
    let tid = Uuid::new_v4();
    let (_token, _rx, _guard) = register(&threads, tid);

    let cause = {
        let map = threads.lock().unwrap();
        map.get(&tid).unwrap().cancel_cause.clone()
    };
    *cause.lock().unwrap() = Some(crate::engine::thread_events::CancelCause::SupersededByFollowup);

    assert!(
        cause.lock().unwrap().take().is_some(),
        "the first read sees the redirect"
    );
    assert!(
        cause.lock().unwrap().take().is_none(),
        "the second read must not: a stale cause would relabel the next turn"
    );
}
