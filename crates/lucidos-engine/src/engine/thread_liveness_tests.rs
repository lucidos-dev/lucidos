use super::*;

const QUIET: i64 = QUIET_AFTER.as_secs() as i64;
const GRACE: i64 = HANDOFF_START_GRACE.as_secs() as i64;

fn facts(status: ThreadStatus, age: Option<i64>) -> LivenessFacts {
    LivenessFacts {
        status,
        last_activity_age_secs: age,
        pending_handoff: None,
        waits: false,
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
fn every_state_names_its_wire_text() {
    for state in LivenessState::ALL {
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            serde_json::Value::String(state.as_str().into())
        );
    }
}

mod db {
    use super::super::*;
    use super::{GRACE, QUIET};
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{
        seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db,
    };

    async fn emit(bus: &EventBus, thread_id: Uuid, event: serde_json::Value) {
        let event: ThreadEvent = serde_json::from_value(event).expect("a thread event");
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
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
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "ResponseGenerated", "text": "Handed off."}),
        )
        .await;
        emit(
            &bus,
            thread,
            serde_json::json!({"type": "CodingAgentIdled"}),
        )
        .await;
        let between = read(&pool, thread).await;
        assert_eq!(between.status, ThreadStatus::Idle);
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
