use super::*;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::chat::rerun::RerunExt;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{CancelCause, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_hold::may_start_turn;
use crate::test_support::{seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db};

async fn stop(bus: &EventBus, thread_id: Uuid) {
    crate::engine::thread_events::emit_response_canceled(
        bus,
        bus.pool(),
        thread_id,
        CancelCause::UserStop,
        String::new(),
        vec![],
        None,
        None,
        EventMeta::with_actor(Some(MessageOrigin::Device {
            device_id: "test-device".into(),
        })),
        "[ThreadStopTest] Stop",
    )
    .await;
}

#[test]
fn only_device_resume_events_release_a_stop() {
    use serde_json::json;
    let device = json!({"actor": {"kind": "device", "device_id": "test-device"}});
    for event in [
        "ContinuationRequested",
        "ContinuationStarted",
        "UserQuestionAnswered",
        "ApplyNowStarted",
        "ChangeApplied",
    ] {
        assert_eq!(transition(event, &device), Some(false), "{event}");
        assert_eq!(transition(event, &json!({})), None, "{event}");
        assert_eq!(
            transition(
                event,
                &json!({"actor": {"kind": "thread_link", "mode": "agent"}})
            ),
            None
        );
    }
    assert_eq!(
        transition(
            "MessageReceived",
            &json!({"mode":"human", "origin":{"kind":"device"}})
        ),
        Some(false)
    );
    for event in [
        "PromptInjected",
        "EventWaitDelivered",
        "ChildThreadCompleted",
        "ResponseGenerated",
        "CodingAgentInputRead",
    ] {
        assert_eq!(transition(event, &device), None, "{event}");
    }
    assert_eq!(
        transition(
            "ResponseCanceled",
            &json!({"cause":"user_stop", "actor":{"kind":"device"}})
        ),
        Some(true)
    );
    assert_eq!(
        transition(
            "ResponseCanceled",
            &json!({"cause":"user_stop", "actor":{"kind":"thread_link"}})
        ),
        None
    );
    assert_eq!(
        transition(
            "ResponseCanceled",
            &json!({"cause":"superseded_by_followup", "actor":{"kind":"device"}})
        ),
        None
    );
}

#[tokio::test]
async fn stop_holds_every_automatic_step_on_both_lanes_across_readers() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for tid in [
        seed_chat_thread(&bus, "hello").await,
        seed_coding_thread(&bus, None).await,
    ] {
        assert!(!thread_is_stopped(&pool, tid).await);
        stop(&bus, tid).await;
        assert!(thread_is_stopped(&pool, tid).await);
        let fresh = pool.clone();
        assert!(thread_is_stopped(&fresh, tid).await);
        assert!(!may_start_turn(&pool, tid).await);
        let asked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'UserQuestionAsked'",
        )
        .bind(tid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(asked, 0, "Stop does not ask permission to wake");
        let mut message = crate::test_support::message_received("resume");
        if let ThreadEvent::MessageReceived { origin, .. } = &mut message {
            *origin = Some(MessageOrigin::Device {
                device_id: "test-device".into(),
            });
        }
        crate::test_support::seed_thread_event(&bus, tid, message).await;
        assert!(!thread_is_stopped(&pool, tid).await);
    }
    teardown_test_db(&db).await;
}
#[tokio::test]
async fn engine_reentries_never_start_a_turn_on_either_stopped_lane() {
    use crate::engine::chat::process::OrphanLane;
    use crate::engine::thread_events::ActorMode;
    use crate::engine::PreEmittedOrigin;
    use crate::test_support::{boot_mock_engine, in_engine_child};
    if !in_engine_child(
        "engine::thread_stop::tests::engine_reentries_never_start_a_turn_on_either_stopped_lane",
    )
    .await
    {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    for tid in [
        seed_chat_thread(bus, "hello").await,
        seed_coding_thread(bus, None).await,
    ] {
        stop(bus, tid).await;
        let fence: i64 =
            sqlx::query_scalar("SELECT max(sequence) FROM events WHERE thread_id = $1")
                .bind(tid)
                .fetch_one(&pool)
                .await
                .unwrap();
        for origin in [
            PreEmittedOrigin::WaitReentry(Uuid::new_v4()),
            PreEmittedOrigin::EngineReentry(Uuid::new_v4()),
            PreEmittedOrigin::KeepAlive(Uuid::new_v4()),
        ] {
            engine
                .process_orphan(
                    "engine wake",
                    None,
                    tid,
                    None,
                    ActorMode::Engine,
                    Some(origin),
                    OrphanLane::LucidosAgent(None),
                )
                .await
                .unwrap();
        }
        let starts: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1 AND sequence > $2 AND event_type IN ('CodingAgentPromptSent','CodingAgentInputRead','ResponseGenerated')")
            .bind(tid).bind(fence).fetch_one(&pool).await.unwrap();
        assert_eq!(starts, 0);
        assert!(thread_is_stopped(&pool, tid).await);
    }
    let tid = seed_chat_thread(bus, "hello").await;
    stop(bus, tid).await;
    let mut message = crate::test_support::message_received("parent follow-up");
    if let ThreadEvent::MessageReceived { mode, .. } = &mut message {
        *mode = ActorMode::Agent;
    }
    let anchor = crate::test_support::seed_thread_event(bus, tid, message).await;
    engine
        .process_orphan(
            "parent follow-up",
            None,
            tid,
            None,
            ActorMode::Agent,
            Some(PreEmittedOrigin::Message(anchor)),
            OrphanLane::LucidosAgent(None),
        )
        .await
        .unwrap();
    let answers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ResponseGenerated'",
    )
    .bind(tid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(answers > 0, "the parent's follow-up must start a turn");
    assert!(
        thread_is_stopped(&pool, tid).await,
        "an agent message does not lift Stop"
    );
    let mut message = crate::test_support::message_received("parent follow-up again");
    if let ThreadEvent::MessageReceived { mode, .. } = &mut message {
        *mode = ActorMode::Agent;
    }
    let start = crate::test_support::seed_thread_event(bus, tid, message).await;
    crate::engine::thread_events::emit_response_aborted(
        bus,
        tid,
        crate::engine::thread_events::AbortCause::EngineShutdown,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            request_event_id: Some(start),
            actor: Some(MessageOrigin::Device {
                device_id: "test-device".into(),
            }),
            ..EventMeta::NONE
        },
        "[ThreadStopTest] switch",
    )
    .await;
    engine
        .continue_chat(tid, None, Default::default())
        .await
        .unwrap();
    let resumed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ContinuationStarted'",
    )
    .bind(tid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(resumed, 0, "boot recovery cannot resume a stopped thread");
    let tid = seed_coding_thread(bus, None).await;
    let report = crate::test_support::seed_thread_event(
        bus,
        tid,
        ThreadEvent::ChildThreadCompleted {
            child_thread_id: Uuid::new_v4(),
            child_thread_title: Some("Build".into()),
            status: crate::engine::thread_events::ChildCompletionStatus::Success,
            summary: "The child finished".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        },
    )
    .await;
    stop(bus, tid).await;
    assert!(held_inputs(&pool, tid).await.unwrap().is_empty());
    let mut message = crate::test_support::message_received("resume");
    if let ThreadEvent::MessageReceived { origin, .. } = &mut message {
        *origin = Some(MessageOrigin::Device {
            device_id: "test-device".into(),
        });
    }
    let human = crate::test_support::seed_thread_event(bus, tid, message).await;
    let mut claim = engine
        .claim_thread_for_spawn(tid, None)
        .await
        .unwrap()
        .unwrap();
    {
        let mut sessions = engine.agent_sessions.lock().await;
        let session = sessions.get_mut(&tid).unwrap();
        session
            .msg_tx
            .send(crate::engine::AgentUserInput {
                text: "resume".into(),
                images: None,
                origin_event_id: Some(human),
                kind: crate::engine::AgentInputKind::User,
                completion: None,
            })
            .unwrap();
        queue_held_inputs(session, held_inputs(&pool, tid).await.unwrap(), |_| false);
        queue_held_inputs(session, held_inputs(&pool, tid).await.unwrap(), |_| false);
    }
    assert_eq!(
        claim.entry.msg_rx.try_recv().unwrap().origin_event_id,
        Some(human)
    );
    let card = claim.entry.msg_rx.try_recv().unwrap();
    assert_eq!(card.origin_event_id, Some(report));
    assert!(card.text.contains("The child finished"));
    assert!(
        claim.entry.msg_rx.try_recv().is_err(),
        "each report queues once"
    );
    crate::test_support::seed_thread_event(
        bus,
        tid,
        ThreadEvent::CodingAgentInputRead {
            input_event_id: report,
            started_turn: false,
            verbatim: false,
        },
    )
    .await;
    assert!(held_inputs(&pool, tid).await.unwrap().is_empty());
    drop(claim);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn unreadable_stop_state_holds() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    pool.close().await;
    assert!(thread_is_stopped(&pool, Uuid::new_v4()).await);
}

#[tokio::test]
async fn a_human_resume_carries_held_waits_after_its_words_once() {
    use crate::engine::thread_events::{ActorMode, EngineReason, EventWaitOutcome};
    use crate::test_support::seed_thread_event;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for tid in [
        seed_chat_thread(&bus, "hello").await,
        seed_coding_thread(&bus, None).await,
    ] {
        stop(&bus, tid).await;
        seed_thread_event(
            &bus,
            tid,
            ThreadEvent::PromptInjected {
                text: "The build passed".into(),
                mode: ActorMode::Agent,
                origin: Some(MessageOrigin::engine(EngineReason::EventWait {
                    outcome: EventWaitOutcome::Delivered,
                    watched: vec![],
                    wait_reason: "build".into(),
                })),
                injected_message_id: None,
                delivered_event_id: None,
            },
        )
        .await;
        assert_eq!(
            resume_text(&pool, tid, "agent follow-up", None)
                .await
                .unwrap(),
            "agent follow-up"
        );
        let mut message = crate::test_support::message_received("resume now");
        if let ThreadEvent::MessageReceived { origin, .. } = &mut message {
            *origin = Some(MessageOrigin::Device {
                device_id: "test-device".into(),
            });
        }
        let resume = seed_thread_event(&bus, tid, message).await;
        assert_eq!(
            resume_text(&pool, tid, "resume now", Some(resume))
                .await
                .unwrap(),
            "resume now\n\n---\n\nThe build passed"
        );
        bus.emit(BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::ResponseGenerated {
                text: "done".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta {
                request_event_id: Some(resume),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        assert_eq!(
            resume_text(&pool, tid, "next message", Some(resume))
                .await
                .unwrap(),
            "next message"
        );
        seed_thread_event(
            &bus,
            tid,
            ThreadEvent::PromptInjected {
                text: "Already handled".into(),
                mode: ActorMode::Agent,
                origin: Some(MessageOrigin::engine(EngineReason::EventWait {
                    outcome: EventWaitOutcome::Delivered,
                    watched: vec![],
                    wait_reason: "old build".into(),
                })),
                injected_message_id: None,
                delivered_event_id: None,
            },
        )
        .await;
        bus.emit(BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::ResponseGenerated {
                text: "handled".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta {
                request_event_id: Some(resume),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        stop(&bus, tid).await;
        let mut message = crate::test_support::message_received("resume again");
        if let ThreadEvent::MessageReceived { origin, .. } = &mut message {
            *origin = Some(MessageOrigin::Device {
                device_id: "test-device".into(),
            });
        }
        let resume = seed_thread_event(&bus, tid, message).await;
        assert_eq!(
            resume_text(&pool, tid, "resume again", Some(resume))
                .await
                .unwrap(),
            "resume again"
        );
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn messages_can_run_while_automatic_input_stays_held() {
    use crate::engine::thread_events::ActorMode;
    use crate::engine::{AgentInputKind as K, RedeliveredInput as R};
    for kind in [
        K::User,
        K::Redelivered {
            sent_at: chrono::Utc::now(),
            input: R::Message {
                sender: ActorMode::Agent,
            },
        },
    ] {
        assert!(!is_automatic_input(kind));
    }
    for kind in [
        K::Continuation,
        K::ReentryFromEngine,
        K::CeilingReentry,
        K::Redelivered {
            sent_at: chrono::Utc::now(),
            input: R::ChildReport,
        },
        K::Redelivered {
            sent_at: chrono::Utc::now(),
            input: R::EnginePrompt,
        },
    ] {
        assert!(is_automatic_input(kind));
    }
}

#[tokio::test]
async fn repeated_stops_preserve_unread_waits_and_child_cards() {
    use crate::engine::thread_events::{ActorMode, EngineReason, EventWaitOutcome};
    use crate::test_support::{seed_thread_event, start_cc_session};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for (startup_finished, terminal_after_resume) in [(false, false), (true, false), (true, true)] {
        let tid = seed_chat_thread(&bus, "hello").await;
        if startup_finished {
            start_cc_session(&bus, tid, "test-branch", None).await;
        }
        stop(&bus, tid).await;
        let wait = seed_thread_event(
            &bus,
            tid,
            ThreadEvent::PromptInjected {
                text: "Held build result".into(),
                mode: ActorMode::Agent,
                origin: Some(MessageOrigin::engine(EngineReason::EventWait {
                    outcome: EventWaitOutcome::Delivered,
                    watched: vec![],
                    wait_reason: "build".into(),
                })),
                injected_message_id: None,
                delivered_event_id: None,
            },
        )
        .await;
        let report = seed_thread_event(
            &bus,
            tid,
            ThreadEvent::ChildThreadCompleted {
                child_thread_id: Uuid::new_v4(),
                child_thread_title: Some("Build".into()),
                status: crate::engine::thread_events::ChildCompletionStatus::Success,
                summary: "Held child report".into(),
                pending_change_ids: vec![],
                sub_thread_pending_changes: vec![],
                integrated_branch: None,
            },
        )
        .await;
        assert!(held_inputs(&pool, tid).await.unwrap().is_empty());
        let mut agent = crate::test_support::message_received("parent follow-up");
        if let ThreadEvent::MessageReceived { mode, .. } = &mut agent {
            *mode = ActorMode::Agent;
        }
        let agent = seed_thread_event(&bus, tid, agent).await;
        start_cc_session(&bus, tid, "test-branch", None).await;
        let response = || BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::ResponseGenerated {
                text: "Answered the parent".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta {
                request_event_id: Some(agent),
                ..EventMeta::NONE
            },
        };
        if !terminal_after_resume {
            bus.emit(response()).await.unwrap();
            assert!(thread_is_stopped(&pool, tid).await);
            stop(&bus, tid).await;
        }
        let mut human = crate::test_support::message_received("resume");
        if let ThreadEvent::MessageReceived { origin, .. } = &mut human {
            *origin = Some(MessageOrigin::Device {
                device_id: "test-device".into(),
            });
        }
        let resume = seed_thread_event(&bus, tid, human).await;
        if terminal_after_resume {
            bus.emit(response()).await.unwrap();
        }
        let held = held_inputs(&pool, tid).await.unwrap();
        assert!(held.iter().any(|input| input.origin_event_id == Some(wait)));
        assert!(held
            .iter()
            .any(|input| input.origin_event_id == Some(report)));
        assert_eq!(
            resume_text(&pool, tid, "resume", Some(resume))
                .await
                .unwrap(),
            "resume\n\n---\n\nHeld build result"
        );
    }
    teardown_test_db(&db).await;
}

async fn human_message(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    let mut message = crate::test_support::message_received(text);
    if let ThreadEvent::MessageReceived { origin, .. } = &mut message {
        *origin = Some(MessageOrigin::Device {
            device_id: "test-device".into(),
        });
    }
    crate::test_support::seed_thread_event(bus, thread_id, message).await
}

async fn wake(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    use crate::engine::thread_events::{ActorMode, EngineReason, EventWaitOutcome};
    crate::test_support::seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::PromptInjected {
            text: text.into(),
            mode: ActorMode::Agent,
            origin: Some(MessageOrigin::engine(EngineReason::EventWait {
                outcome: EventWaitOutcome::Delivered,
                watched: vec![],
                wait_reason: "build".into(),
            })),
            injected_message_id: None,
            delivered_event_id: None,
        },
    )
    .await
}

#[tokio::test]
async fn a_wake_drained_live_is_never_attached_to_a_resume() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = seed_chat_thread(&bus, "hello").await;
    human_message(&bus, tid, "first").await;
    stop(&bus, tid).await;
    let resume = human_message(&bus, tid, "resume").await;
    assert_eq!(
        resume_text(&pool, tid, "resume", Some(resume))
            .await
            .unwrap(),
        "resume"
    );
    wake(&bus, tid, "Main check failed").await;
    let next = human_message(&bus, tid, "next").await;
    assert_eq!(
        resume_text(&pool, tid, "next", Some(next)).await.unwrap(),
        "next",
        "only the first resume after a Stop carries held wakes"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_held_wake_is_attached_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = seed_chat_thread(&bus, "hello").await;
    stop(&bus, tid).await;
    wake(&bus, tid, "Change applied").await;
    let first = human_message(&bus, tid, "first").await;
    let second = human_message(&bus, tid, "second").await;
    assert_eq!(
        resume_text(&pool, tid, "first", Some(first)).await.unwrap(),
        "first\n\n---\n\nChange applied"
    );
    assert_eq!(
        resume_text(&pool, tid, "second", Some(second))
            .await
            .unwrap(),
        "second"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_wake_that_ran_its_own_turn_was_never_held() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = seed_chat_thread(&bus, "hello").await;
    stop(&bus, tid).await;
    let ran = wake(&bus, tid, "Ran on its own").await;
    bus.emit(BusEvent::Thread {
        thread_id: tid,
        event: ThreadEvent::ChatTurnSettingsResolved {
            model: "mock".into(),
            reasoning_effort: None,
            provider: None,
        },
        meta: EventMeta {
            request_event_id: Some(ran),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    wake(&bus, tid, "Held").await;
    let resume = human_message(&bus, tid, "resume").await;
    assert_eq!(
        resume_text(&pool, tid, "resume", Some(resume))
            .await
            .unwrap(),
        "resume\n\n---\n\nHeld"
    );
    teardown_test_db(&db).await;
}

async fn reply(bus: &EventBus, thread_id: Uuid, starter: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            request_event_id: Some(starter),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn the_stop_hold_scan_ignores_history_before_the_latest_stop_run() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = seed_chat_thread(&bus, "hello").await;
    stop(&bus, tid).await;
    for n in 0..200 {
        wake(&bus, tid, &format!("old wake {n}")).await;
    }
    let old_resume = human_message(&bus, tid, "old resume").await;
    reply(&bus, tid, old_resume).await;
    stop(&bus, tid).await;
    wake(&bus, tid, "new wake").await;
    let resume = human_message(&bus, tid, "resume").await;
    let started = std::time::Instant::now();
    assert_eq!(
        resume_text(&pool, tid, "resume", Some(resume))
            .await
            .unwrap(),
        "resume\n\n---\n\nnew wake"
    );
    let not_stopped = human_message(&bus, tid, "plain").await;
    assert_eq!(
        resume_text(&pool, tid, "plain", Some(not_stopped))
            .await
            .unwrap(),
        "plain"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the scan costs a fixed number of queries, whatever the thread's history"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_wake_recorded_just_before_a_stop_rides_on_the_resume() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = seed_chat_thread(&bus, "hello").await;
    let first = human_message(&bus, tid, "first").await;
    reply(&bus, tid, first).await;
    wake(&bus, tid, "Raced the Stop").await;
    stop(&bus, tid).await;
    let resume = human_message(&bus, tid, "resume").await;
    assert_eq!(
        resume_text(&pool, tid, "resume", Some(resume))
            .await
            .unwrap(),
        "resume\n\n---\n\nRaced the Stop"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_carrier_taken_back_leaves_its_wakes_for_the_next_resume() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = seed_chat_thread(&bus, "hello").await;
    stop(&bus, tid).await;
    wake(&bus, tid, "Held build result").await;
    let carrier = human_message(&bus, tid, "carrier").await;
    assert_eq!(
        resume_text(&pool, tid, "carrier", Some(carrier))
            .await
            .unwrap(),
        "carrier\n\n---\n\nHeld build result"
    );
    crate::test_support::seed_thread_event(
        &bus,
        tid,
        ThreadEvent::QueuedMessageRemoved {
            removed_message_id: carrier,
            restore_to_compose: false,
            compose: None,
        },
    )
    .await;
    assert!(
        thread_is_stopped(&pool, tid).await,
        "a message the user took back does not lift Stop"
    );
    let resend = human_message(&bus, tid, "resend").await;
    assert_eq!(
        resume_text(&pool, tid, "resend", Some(resend))
            .await
            .unwrap(),
        "resend\n\n---\n\nHeld build result"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_carrier_stopped_before_its_reply_leaves_its_wakes_for_the_next_resume() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = seed_chat_thread(&bus, "hello").await;
    stop(&bus, tid).await;
    wake(&bus, tid, "Held build result").await;
    let carrier = human_message(&bus, tid, "carrier").await;
    resume_text(&pool, tid, "carrier", Some(carrier))
        .await
        .unwrap();
    stop(&bus, tid).await;
    let next = human_message(&bus, tid, "next").await;
    assert_eq!(
        resume_text(&pool, tid, "next", Some(next)).await.unwrap(),
        "next\n\n---\n\nHeld build result"
    );
    reply(&bus, tid, next).await;
    stop(&bus, tid).await;
    let last = human_message(&bus, tid, "last").await;
    assert_eq!(
        resume_text(&pool, tid, "last", Some(last)).await.unwrap(),
        "last"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_coding_agent_keeps_an_unread_delivery_across_a_second_stop_run() {
    use crate::test_support::start_cc_session;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = seed_coding_thread(&bus, None).await;
    start_cc_session(&bus, tid, "test-branch", None).await;
    stop(&bus, tid).await;
    let held = wake(&bus, tid, "Held build result").await;
    human_message(&bus, tid, "resume").await;
    start_cc_session(&bus, tid, "test-branch", None).await;
    stop(&bus, tid).await;
    human_message(&bus, tid, "resume again").await;
    let inputs = held_inputs(&pool, tid).await.unwrap();
    assert!(inputs
        .iter()
        .any(|input| input.origin_event_id == Some(held)));
    teardown_test_db(&db).await;
}
