use super::*;
use crate::engine::thread_lifecycle::ThreadStatus;

const QUIET: i64 = QUIET_AFTER.as_secs() as i64;
const GRACE: i64 = HANDOFF_START_GRACE.as_secs() as i64;

fn facts(status: ThreadStatus, age: Option<i64>) -> LivenessFacts {
    LivenessFacts {
        status,
        last_activity_age_secs: age,
        pending_handoff: None,
        waits: false,
        between_turns: false,
        queued: false,
        held: false,
    }
}

fn with_handoff(mut f: LivenessFacts, starts: SessionStart) -> LivenessFacts {
    f.pending_handoff = Some(starts);
    f
}

/// The table in the plan, row by row, with each threshold's edge.
#[test]
fn derive_follows_the_table_first_match_wins() {
    use LivenessState as L;
    use ThreadStatus as S;
    let waiting = LivenessFacts {
        waits: true,
        ..facts(S::Idle, Some(5))
    };
    let cases = [
        (facts(S::Running, Some(QUIET - 1)), L::Working),
        (facts(S::Running, Some(QUIET)), L::Quiet),
        (facts(S::Running, None), L::Working),
        (
            with_handoff(facts(S::Running, Some(QUIET)), SessionStart::Now),
            L::Quiet,
        ),
        (
            facts(S::WaitingForUserAnswer, Some(QUIET)),
            L::WaitingForUserAnswer,
        ),
        (facts(S::Paused, Some(GRACE)), L::Paused),
        (
            with_handoff(facts(S::Failed, Some(GRACE)), SessionStart::Now),
            L::Failed,
        ),
        (
            with_handoff(facts(S::Idle, Some(GRACE - 1)), SessionStart::Now),
            L::Working,
        ),
        (
            with_handoff(facts(S::Idle, Some(GRACE)), SessionStart::Now),
            L::Stalled,
        ),
        (
            with_handoff(facts(S::Idle, Some(GRACE)), SessionStart::AfterThisTurn),
            L::Stalled,
        ),
        (
            with_handoff(facts(S::Waiting, Some(GRACE)), SessionStart::Now),
            L::Stalled,
        ),
        (
            with_handoff(facts(S::Idle, Some(GRACE * 10)), SessionStart::OnNextInput),
            L::Idle,
        ),
        (with_handoff(waiting, SessionStart::OnNextInput), L::Waiting),
        (
            with_handoff(facts(S::Idle, None), SessionStart::Now),
            L::Working,
        ),
        (waiting, L::Waiting),
        (facts(S::Waiting, Some(5)), L::Idle),
        (facts(S::Idle, Some(GRACE * 100)), L::Idle),
    ];
    for (input, want) in cases {
        assert_eq!(derive(&input), want, "{input:?}");
    }
}

/// `stalled` comes from one shape only: an unprompted pending handoff on a
/// thread with no turn in flight, past the grace.
#[test]
fn only_an_unprompted_handoff_past_the_grace_is_stalled() {
    let ages = [
        None,
        Some(0),
        Some(GRACE - 1),
        Some(GRACE),
        Some(QUIET * 10),
    ];
    let handoffs = [
        None,
        Some(SessionStart::Now),
        Some(SessionStart::AfterThisTurn),
        Some(SessionStart::OnNextInput),
    ];
    for status in ThreadStatus::ALL {
        for age in ages {
            for pending_handoff in handoffs {
                for waits in [false, true] {
                    let f = LivenessFacts {
                        status,
                        last_activity_age_secs: age,
                        pending_handoff,
                        waits,
                        between_turns: false,
                        queued: false,
                        held: false,
                    };
                    let stalled = matches!(status, ThreadStatus::Idle | ThreadStatus::Waiting)
                        && pending_handoff.is_some_and(SessionStart::is_unprompted)
                        && age.is_some_and(|a| a >= GRACE);
                    assert_eq!(derive(&f) == LivenessState::Stalled, stalled, "{f:?}");
                }
            }
        }
    }
}

#[test]
fn running_successor_gap_can_stall_but_an_active_turn_only_goes_quiet() {
    let active = with_handoff(facts(ThreadStatus::Running, Some(QUIET)), SessionStart::Now);
    assert_eq!(derive(&active), LivenessState::Quiet);
    let gap = LivenessFacts {
        between_turns: true,
        ..active
    };
    assert_eq!(derive(&gap), LivenessState::Stalled);
    assert_eq!(
        derive(&LivenessFacts {
            last_activity_age_secs: Some(GRACE - 1),
            ..gap
        }),
        LivenessState::Working
    );
}

#[test]
fn queued_and_held_respect_turn_and_handoff_precedence() {
    let waiting = LivenessFacts {
        waits: true,
        ..facts(ThreadStatus::Idle, Some(GRACE))
    };
    let held = LivenessFacts {
        held: true,
        ..waiting
    };
    let queued = LivenessFacts {
        queued: true,
        ..held
    };
    assert_eq!(derive(&held), LivenessState::Held);
    assert_eq!(derive(&queued), LivenessState::Queued);
    assert_eq!(
        derive(&LivenessFacts {
            status: ThreadStatus::Running,
            ..queued
        }),
        LivenessState::Working
    );
    assert_eq!(
        derive(&LivenessFacts {
            status: ThreadStatus::WaitingForUserAnswer,
            ..queued
        }),
        LivenessState::WaitingForUserAnswer
    );
    assert_eq!(
        derive(&LivenessFacts {
            status: ThreadStatus::Running,
            waits: false,
            between_turns: true,
            held: true,
            ..with_handoff(waiting, SessionStart::Now)
        }),
        LivenessState::Held
    );
}

#[test]
fn every_state_names_its_wire_text() {
    for state in LivenessState::ALL {
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            serde_json::Value::String(state.as_str().into())
        );
    }
}

mod db {
    use super::*;
    use super::{GRACE, QUIET};
    use crate::engine::agent_session::spawn_claim::SpawnClaimExt;
    use crate::engine::claude_code::control::ControlExt;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{
        seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db,
    };

    async fn emit(bus: &EventBus, thread_id: Uuid, event: serde_json::Value) {
        let actor = serde_json::from_value(event["actor"].clone()).expect("valid event actor");
        let event: ThreadEvent = serde_json::from_value(event).expect("a thread event");
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                actor,
                ..EventMeta::NONE
            },
        })
        .await
        .expect("emit");
    }

    fn handed_off(starts: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "SessionHandedOff",
            "starts": starts,
            "opening": "narrative",
            "message": "A fresh session starts here.",
            "checkpoint": "plan-approval",
            "coding_agent": "claude-code",
        })
    }

    fn tool_result(result: &str) -> serde_json::Value {
        serde_json::json!({"type": "CodingAgentToolResult", "name": "Bash", "result": result})
    }

    fn session_started(kind: &str) -> serde_json::Value {
        let continuity = if kind == "handoff" {
            serde_json::json!({"kind": "handoff", "opening": "narrative", "message": "A fresh session starts here."})
        } else {
            serde_json::json!({"kind": kind})
        };
        serde_json::json!({
            "type": "SessionStarted",
            "coding_agent": "claude-code",
            "session_id": "test-session",
            "branch": "claude-code/test",
            "coding_agent_folder": "",
            "continuity": continuity,
        })
    }

    async fn backdate(pool: &PgPool, thread_id: Uuid, secs: i64) {
        sqlx::query(
            "UPDATE events SET created = created - make_interval(secs => $2) WHERE thread_id = $1",
        )
        .bind(thread_id)
        .bind(secs as f64)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn read(pool: &PgPool, thread_id: Uuid) -> ThreadLiveness {
        liveness_for_thread(pool, thread_id)
            .await
            .unwrap()
            .expect("a summary row")
    }

    async fn idle(bus: &EventBus, thread: Uuid) {
        emit(bus, thread, serde_json::json!({"type": "CodingAgentIdled"})).await;
    }

    async fn register_wait(
        bus: &EventBus,
        thread: Uuid,
        reason: &str,
    ) -> chrono::DateTime<chrono::Utc> {
        let expires_at = chrono::DateTime::from_timestamp(2_000_000_000, 0).unwrap();
        let mut event =
            serde_json::to_value(crate::test_support::event_wait_started(Uuid::new_v4())).unwrap();
        event["reason"] = reason.into();
        event["expires_at"] = serde_json::json!(expires_at);
        if reason == "the build" {
            event["background_timing"] = serde_json::json!({"state": "queued"});
        }
        emit(bus, thread, event).await;
        expires_at
    }

    #[tokio::test]
    async fn liveness_names_event_waits_in_registration_order() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let expires_at = register_wait(&bus, thread, "the build").await;
        register_wait(&bus, thread, "the tests").await;
        idle(&bus, thread).await;

        let liveness = read(&pool, thread).await;
        assert_eq!(liveness.state, LivenessState::Waiting);
        assert_eq!(liveness.status, ThreadStatus::Idle);
        let json = serde_json::to_value(&liveness).unwrap();
        assert_eq!(
            json["live_event_waits"],
            serde_json::json!([
                {"reason": "the build", "on": [{"event_type": "BackgroundBashCompleted"}],
                 "expires_at": expires_at, "expires_in_secs": (expires_at - liveness.as_of).num_seconds(),
                 "background_timing": {"state": "queued"}},
                {"reason": "the tests", "on": [{"event_type": "BackgroundBashCompleted"}],
                 "expires_at": expires_at, "expires_in_secs": (expires_at - liveness.as_of).num_seconds()},
            ])
        );
        assert!(json.get("awaited_children").is_none());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn liveness_names_awaited_children() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = seed_coding_thread(&bus, None).await;
        let child = seed_coding_thread(&bus, Some(parent)).await;
        idle(&bus, parent).await;

        let liveness = read(&pool, parent).await;
        assert_eq!(liveness.state, LivenessState::Waiting);
        let json = serde_json::to_value(liveness).unwrap();
        assert_eq!(
            json["awaited_children"],
            serde_json::json!([{"thread_id": child, "title": "work"}])
        );
        idle(&bus, child).await;
        let resumed = read(&pool, parent).await;
        assert_eq!(resumed.state, LivenessState::Working);
        let json = serde_json::to_value(resumed).unwrap();
        for key in [
            "live_event_waits",
            "awaited_children",
            "expected_wake",
            "hold",
        ] {
            assert!(json.get(key).is_none(), "{json}");
        }
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn liveness_only_shows_an_expected_wake_from_the_current_turn() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let at = register_wait(&bus, thread, "the build").await;
        let wake = serde_json::json!({
            "type": "WakeExpected", "at": at, "source": "agent", "reason": "build should finish",
            "distribution": [{"by": at, "probability": 1.0}],
        });
        emit(&bus, thread, wake.clone()).await;
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "ContinuationRequested"}),
        )
        .await;
        idle(&bus, thread).await;
        let stale = serde_json::to_value(read(&pool, thread).await).unwrap();
        assert_eq!(stale["state"], "waiting");
        assert!(stale.get("expected_wake").is_none());
        emit(&bus, thread, wake).await;
        let current = serde_json::to_value(read(&pool, thread).await).unwrap();
        assert_eq!(
            current["expected_wake"],
            serde_json::json!({
                "at": at, "source": "agent", "reason": "build should finish",
            })
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_child_wake_supersedes_the_parents_old_expected_wake() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = seed_coding_thread(&bus, None).await;
        let child = seed_coding_thread(&bus, Some(parent)).await;
        let at = register_wait(&bus, parent, "the build").await;
        emit(
            &bus,
            parent,
            serde_json::json!({
                "type": "WakeExpected", "at": at, "source": "agent", "reason": "the build",
            }),
        )
        .await;
        idle(&bus, parent).await;
        assert!(read(&pool, parent).await.expected_wake.is_some());

        idle(&bus, child).await;
        assert_eq!(read(&pool, parent).await.state, LivenessState::Working);
        idle(&bus, parent).await;

        let live = read(&pool, parent).await;
        assert_eq!(live.state, LivenessState::Waiting);
        assert_eq!(live.live_event_waits.len(), 1);
        assert!(live.expected_wake.is_none(), "{live:?}");
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn automation_pause_and_stop_hold_live_waits() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let at = register_wait(&bus, thread, "the build").await;
        emit(&bus, thread, serde_json::json!({
            "type": "WakeExpected", "at": at, "source": "agent", "reason": "build should finish",
        })).await;
        idle(&bus, thread).await;

        emit(
            &bus,
            thread,
            serde_json::json!({"type": "AutomationPaused", "reason": "check the build"}),
        )
        .await;
        let paused = serde_json::to_value(read(&pool, thread).await).unwrap();
        assert_eq!(paused["state"], "held");
        assert_eq!(
            paused["hold"],
            serde_json::json!({"by": "automation_pause", "reason": "check the build"})
        );
        assert_eq!(paused["expected_wake"]["at"], serde_json::json!(at));

        emit(
            &bus,
            thread,
            serde_json::json!({
                "type": "ResponseCanceled", "cause": "user_stop",
                "actor": {"kind": "device", "device_id": "test-device"},
            }),
        )
        .await;
        let stopped = serde_json::to_value(read(&pool, thread).await).unwrap();
        assert_eq!(stopped["state"], "held");
        assert_eq!(stopped["hold"], serde_json::json!({"by": "stop"}));
        assert_eq!(stopped["live_event_waits"][0]["reason"], "the build");

        emit(
            &bus,
            thread,
            serde_json::json!({"type": "MessageReceived", "text": "continue", "device_id": "test-device"}),
        )
        .await;
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "AutomationUnpaused", "cause": "unpause"}),
        )
        .await;
        idle(&bus, thread).await;
        let resumed = serde_json::to_value(read(&pool, thread).await).unwrap();
        assert_eq!(resumed["state"], "waiting");
        assert!(resumed.get("hold").is_none());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn liveness_names_an_automation_hold_on_a_successor() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        emit(&bus, thread, handed_off("after-this-turn")).await;
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "AutomationPaused", "reason": "check the build"}),
        )
        .await;
        idle(&bus, thread).await;

        let live = serde_json::to_value(read(&pool, thread).await).unwrap();
        assert_eq!(live["state"], "held");
        assert_eq!(
            live["hold"],
            serde_json::json!({"by": "automation_pause", "reason": "check the build"})
        );
        assert_eq!(live["handoff"], "pending");
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn bound_thread_queue_entry_reads_queued_until_admitted() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        idle(&bus, thread).await;
        sqlx::query("INSERT INTO thread_queue (id, kind, thread_id, request) VALUES ($1, 'coding-agent', $1, '{}')")
            .bind(thread).execute(&pool).await.unwrap();
        assert_eq!(read(&pool, thread).await.state, LivenessState::Queued);
        sqlx::query("UPDATE thread_queue SET status = 'admitted' WHERE id = $1")
            .bind(thread)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(read(&pool, thread).await.state, LivenessState::Idle);
        teardown_test_db(&db).await;
    }

    /// The Film-takes incident, replayed. Mid-handoff, with a transient tool
    /// error as the newest row, the thread is `working`. After its successor
    /// session starts it is `working` in session 2, opened by the handoff.
    #[tokio::test]
    async fn the_incident_reads_working_at_every_point() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "HandoffCheckpointReached", "name": "plan-approval"}),
        )
        .await;
        emit(
            &bus,
            thread,
            tool_result("Error: Could not read the handoff narrative"),
        )
        .await;

        let mid = read(&pool, thread).await;
        assert_eq!(mid.state, LivenessState::Working, "{mid:?}");
        assert_eq!(mid.status, ThreadStatus::Running);
        assert_eq!(mid.handoff, None);
        let session = mid.session.expect("a coding-agent session");
        assert_eq!(
            (session.ordinal, session.continuity),
            (1, SessionOpening::First)
        );

        emit(&bus, thread, handed_off("after-this-turn")).await;
        assert_eq!(read(&pool, thread).await.state, LivenessState::Working);
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Running);
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "ResponseGenerated", "text": "Handed off."}),
        )
        .await;
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Running);
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "CodingAgentIdled"}),
        )
        .await;
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Running);
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "SessionEnded", "reason": "completed"}),
        )
        .await;
        crate::engine::agent_recovery::settle_orphaned_running_threads(
            &pool,
            &bus,
            &std::collections::HashSet::new(),
        )
        .await;
        let between = read(&pool, thread).await;
        assert_eq!(between.status, ThreadStatus::Running);
        assert_eq!(between.state, LivenessState::Working, "{between:?}");
        assert_eq!(between.handoff, Some(LivenessHandoff::Pending));

        emit(
            &bus,
            thread,
            serde_json::json!({"type": "MessageReceived", "text": "the opening"}),
        )
        .await;
        emit(&bus, thread, session_started("handoff")).await;
        emit(&bus, thread, tool_result("ok")).await;
        let after = read(&pool, thread).await;
        assert_eq!(after.state, LivenessState::Working, "{after:?}");
        assert_eq!(after.handoff, None);
        let session = after.session.expect("a coding-agent session");
        assert_eq!(
            (session.ordinal, session.continuity),
            (2, SessionOpening::Handoff)
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn automatic_handoff_recorded_after_idle_revives_the_thread() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "CodingAgentIdled"}),
        )
        .await;
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Idle);
        emit(&bus, thread, handed_off("now")).await;
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Running);
        backdate(&pool, thread, GRACE + 1).await;
        assert_eq!(read(&pool, thread).await.state, LivenessState::Stalled);
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "SessionHandoffWithdrawn"}),
        )
        .await;
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Idle);
        emit(&bus, thread, handed_off("now")).await;
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Running);
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "AutomationPaused", "reason": "owner paused"}),
        )
        .await;
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Idle);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_fresh_handoff_after_failure_starts_but_a_later_failure_blocks_it() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let failed = serde_json::json!({"type": "ResponseFailed", "error": "provider blocked"});
        emit(&bus, thread, failed.clone()).await;
        emit(&bus, thread, handed_off("now")).await;
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Running);
        assert!(
            crate::engine::handoff_state::successor_to_start(&pool, thread)
                .await
                .unwrap()
                .is_some()
        );
        emit(&bus, thread, failed).await;
        assert!(
            crate::engine::handoff_state::successor_to_start(&pool, thread)
                .await
                .unwrap()
                .is_none()
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn stop_in_the_successor_gap_ends_its_pending_handoff() {
        if !crate::test_support::in_engine_child("engine::thread_liveness::tests::db::stop_in_the_successor_gap_ends_its_pending_handoff").await {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
        let thread = seed_coding_thread(&engine.event_bus, None).await;
        emit(&engine.event_bus, thread, handed_off("after-this-turn")).await;
        emit(
            &engine.event_bus,
            thread,
            serde_json::json!({"type": "CodingAgentIdled"}),
        )
        .await;
        assert!(engine
            .interrupt_agent(
                Some(thread),
                None,
                crate::engine::agent_host::SettleTerminal::StuckProjection
            )
            .await
            .unwrap());
        assert_eq!(read(&pool, thread).await.status, ThreadStatus::Idle);
        assert!(
            crate::engine::pending_handoff::pending_handoff(&pool, thread)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            crate::engine::handoff_state::successor_to_start(&pool, thread)
                .await
                .unwrap()
                .is_none()
        );
        // Cleanup callers must not consume an owed successor as a user Stop.
        let cleanup = seed_coding_thread(&engine.event_bus, None).await;
        emit(&engine.event_bus, cleanup, handed_off("after-this-turn")).await;
        emit(
            &engine.event_bus,
            cleanup,
            serde_json::json!({"type": "CodingAgentIdled"}),
        )
        .await;
        assert!(engine
            .settle_unclaimed_running_thread(
                cleanup,
                None,
                crate::engine::agent_host::SettleTerminal::StuckProjection,
            )
            .await
            .unwrap());
        assert!(
            crate::engine::pending_handoff::pending_handoff(&pool, cleanup)
                .await
                .unwrap()
                .is_some()
        );
        engine.pool().close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn successor_bookkeeping_respects_explicit_outcomes() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        for outcome in [
            serde_json::json!({"type": "ResponseCanceled", "cause": "user_stop"}),
            serde_json::json!({"type": "ResponseFailed", "error": "failed"}),
            serde_json::json!({"type": "SessionHandoffWithdrawn"}),
            serde_json::json!({"type": "AutomationPaused", "reason": "owner paused"}),
            serde_json::json!({"type": "UserQuestionAsked", "tool_use_id": "q", "cc_session_id": "s", "question": "Pick?", "options": [], "multi_select": false}),
        ] {
            let thread = seed_coding_thread(&bus, None).await;
            emit(&bus, thread, handed_off("after-this-turn")).await;
            emit(&bus, thread, outcome.clone()).await;
            emit(
                &bus,
                thread,
                serde_json::json!({"type": "CodingAgentIdled"}),
            )
            .await;
            assert_ne!(
                read(&pool, thread).await.status,
                ThreadStatus::Running,
                "{outcome}"
            );
            assert!(
                crate::engine::handoff_state::successor_to_start(&pool, thread)
                    .await
                    .unwrap()
                    .is_none(),
                "{outcome}"
            );
        }
        let next_input = seed_coding_thread(&bus, None).await;
        emit(&bus, next_input, handed_off("on-next-input")).await;
        emit(
            &bus,
            next_input,
            serde_json::json!({"type": "CodingAgentIdled"}),
        )
        .await;
        assert_eq!(read(&pool, next_input).await.status, ThreadStatus::Idle);
        teardown_test_db(&db).await;
    }

    /// An unstarted handoff reads `working` inside the grace and `stalled`
    /// past it. A next-input handoff owes nothing, and a failed start reads
    /// `failed`.
    #[tokio::test]
    async fn a_handoff_is_stalled_only_when_owed_and_past_the_grace() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let idle_handoff = |starts: &'static str| {
            let bus = &bus;
            async move {
                let thread = seed_coding_thread(bus, None).await;
                emit(bus, thread, handed_off(starts)).await;
                emit(bus, thread, serde_json::json!({"type": "CodingAgentIdled"})).await;
                thread
            }
        };

        let owed = idle_handoff("now").await;
        assert_eq!(read(&pool, owed).await.state, LivenessState::Working);
        backdate(&pool, owed, GRACE + 60).await;
        assert_eq!(read(&pool, owed).await.state, LivenessState::Stalled);
        for started_turn in [false, true] {
            emit(
                &bus,
                owed,
                serde_json::json!({
                    "type": "CodingAgentInputRead", "input_event_id": Uuid::new_v4(),
                    "started_turn": started_turn,
                }),
            )
            .await;
            backdate(&pool, owed, QUIET + 1).await;
            assert_eq!(
                read(&pool, owed).await.state,
                if started_turn {
                    LivenessState::Quiet
                } else {
                    LivenessState::Stalled
                }
            );
        }

        let next_input = idle_handoff("on-next-input").await;
        backdate(&pool, next_input, GRACE + 60).await;
        let read_back = read(&pool, next_input).await;
        assert_eq!(read_back.state, LivenessState::Idle);
        assert_eq!(read_back.handoff, Some(LivenessHandoff::Pending));

        let failed = seed_coding_thread(&bus, None).await;
        emit(&bus, failed, handed_off("now")).await;
        emit(
            &bus,
            failed,
            serde_json::json!({"type": "MessageReceived", "text": "the opening"}),
        )
        .await;
        emit(
            &bus,
            failed,
            serde_json::json!({"type": "ResponseFailed", "error": "spawn failed"}),
        )
        .await;
        backdate(&pool, failed, GRACE + 60).await;
        let read_back = read(&pool, failed).await;
        assert_eq!(read_back.state, LivenessState::Failed);
        assert_eq!(read_back.handoff, None, "a failed start ends the handoff");

        teardown_test_db(&db).await;
    }

    /// A handoff below a snapshot restore floor is history, never `stalled`.
    #[tokio::test]
    async fn a_handoff_below_the_restore_floor_is_not_stalled() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        emit(&bus, thread, handed_off("now")).await;
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "CodingAgentIdled"}),
        )
        .await;
        backdate(&pool, thread, GRACE + 60).await;
        assert_eq!(read(&pool, thread).await.state, LivenessState::Stalled);
        sqlx::query(
            "INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) \
             SELECT $1, $2, now(), MAX(sequence) FROM events",
        )
        .bind(Uuid::new_v4())
        .bind(crate::core::snapshot::handle::Handle::generate().as_str())
        .execute(&pool)
        .await
        .unwrap();
        // Restore runs the boot settlement before replaying pending work.
        crate::engine::agent_recovery::settle_orphaned_running_threads(
            &pool,
            &bus,
            &std::collections::HashSet::new(),
        )
        .await;
        let restored = read(&pool, thread).await;
        assert_eq!(restored.state, LivenessState::Idle);
        assert_eq!(restored.handoff, None);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A turn silent past `QUIET_AFTER` reads `quiet`, never `stalled`. A chat
    /// thread has no session. Reading writes nothing.
    #[tokio::test]
    async fn a_silent_turn_is_quiet_and_a_read_writes_nothing() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let coding = seed_coding_thread(&bus, None).await;
        let chat = seed_chat_thread(&bus, "hello").await;
        backdate(&pool, coding, QUIET + 60).await;
        let count = || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events")
                .fetch_one(&pool)
                .await
                .unwrap()
        };
        let before = count().await;

        let all = liveness_for_threads(&pool, &[coding, chat, Uuid::new_v4()])
            .await
            .unwrap();
        assert_eq!(all.len(), 2, "a thread with no summary row is absent");
        let quiet = &all[&coding];
        assert_eq!(quiet.state, LivenessState::Quiet);
        assert!(quiet.last_activity_age_secs.unwrap() >= QUIET);
        let chat = &all[&chat];
        assert_eq!(chat.state, LivenessState::Working);
        assert_eq!(chat.session, None);
        assert!(
            !serde_json::to_string(chat).unwrap().contains("session"),
            "an absent session is omitted"
        );
        assert_eq!(count().await, before, "a read writes no event");

        teardown_test_db(&db).await;
    }
}
