use super::*;
use super::common::*;
use uuid::Uuid;

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
