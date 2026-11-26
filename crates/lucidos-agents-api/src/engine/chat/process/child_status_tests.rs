use super::*;
use crate::engine::thread_liveness::LivenessState;

fn child(title: &str, state: LivenessState, report: Option<i64>) -> ChildStatus {
    ChildStatus {
        title: Some(title.into()),
        liveness: ThreadLiveness {
            state,
            status: crate::engine::thread_lifecycle::ThreadStatus::Idle,
            last_activity_age_secs: Some(30),
            session: None,
            handoff: None,
            live_event_waits: vec![],
            awaited_children: vec![],
            expected_wake: None,
            hold: None,
            as_of: chrono::DateTime::UNIX_EPOCH,
        },
        report_age_secs: report,
    }
}

#[test]
fn no_children_means_no_block() {
    assert_eq!(render_child_status(Ok(&ChildStatuses::default())), "");
}

#[test]
fn a_row_carries_the_state_now_beside_the_last_report() {
    let statuses = ChildStatuses {
        listed: vec![child(
            "Engine crate split plan refresh",
            LivenessState::Working,
            Some(43 * 60),
        )],
        unlisted: 0,
    };
    let block = render_child_status(Ok(&statuses));
    assert!(block.starts_with("[CHILD THREADS]\n"), "{block}");
    assert!(block.ends_with("\n[END CHILD THREADS]"), "{block}");
    assert!(
        block.contains(
            "- \"Engine crate split plan refresh\": working now, active just now; \
             last reported to you 43 min ago"
        ),
        "{block}"
    );
}

#[test]
fn states_read_as_words_and_a_silent_child_says_so() {
    let statuses = ChildStatuses {
        listed: vec![child("Ask", LivenessState::WaitingForUserAnswer, None)],
        unlisted: 0,
    };
    let block = render_child_status(Ok(&statuses));
    assert!(block.contains("waiting for user answer now"), "{block}");
    assert!(block.contains("has not reported to you"), "{block}");
}

#[test]
fn a_held_child_names_its_waits_and_expected_wake() {
    use crate::engine::thread_events::WakeSource;
    use crate::engine::thread_liveness::{
        LivenessAwaitedChild, LivenessEventWait, LivenessExpectedWake,
    };

    let mut child = child("Build", LivenessState::Held, None);
    let at = "2026-11-20T16:00:00Z".parse().unwrap();
    child.liveness.live_event_waits.push(LivenessEventWait {
        reason: "test\nresults".into(),
        on: vec![],
        expires_at: at,
        expires_in_secs: 120,
        background_timing: None,
    });
    child.liveness.awaited_children.push(LivenessAwaitedChild {
        title: "Compile".into(),
        thread_id: Uuid::new_v4(),
    });
    child.liveness.hold = Some(LivenessHold::Hold {
        reason: "review first".into(),
        root_thread_id: Uuid::new_v4(),
    });
    child.liveness.expected_wake = Some(LivenessExpectedWake {
        at,
        source: WakeSource::Agent,
        reason: "suite usually finishes then".into(),
    });
    let statuses = ChildStatuses {
        listed: vec![child],
        unlisted: 0,
    };
    let block = render_child_status(Ok(&statuses));
    for needle in [
        "held now",
        r#"waiting for "test\nresults" (120 s left)"#,
        r#"awaiting child "Compile""#,
        r#"on hold ("review first")"#,
        r#"expected wake 2026-11-20T16:00:00Z (agent: "suite usually finishes then")"#,
    ] {
        assert!(block.contains(needle), "missing {needle}: {block}");
    }
    assert_eq!(block.lines().count(), 3, "{block}");
}

#[test]
fn long_wait_reasons_stay_short_in_the_turn_tail() {
    use crate::engine::thread_liveness::LivenessEventWait;

    let mut child = child("Build", LivenessState::Waiting, None);
    for label in ["build", "tests"] {
        child.liveness.live_event_waits.push(LivenessEventWait {
            reason: format!("{label} {}", "界".repeat(10_000)),
            on: vec![],
            expires_at: chrono::DateTime::UNIX_EPOCH,
            expires_in_secs: 15,
            background_timing: None,
        });
    }
    let block = render_child_status(Ok(&ChildStatuses {
        listed: vec![child],
        unlisted: 0,
    }));
    assert!(block.contains(r#"waiting for "build "#));
    assert!(block.contains(r#"waiting for "tests "#));
    assert_eq!(block.matches("…\" (15 s left)").count(), 2);
    assert!(block.len() < 1_200);
}

#[test]
fn the_rest_collapse_into_one_count_line() {
    let statuses = ChildStatuses {
        listed: (0..CHILD_STATUS_ROWS)
            .map(|i| child(&format!("Child {i}"), LivenessState::Idle, Some(60)))
            .collect(),
        unlisted: 12,
    };
    let block = render_child_status(Ok(&statuses));
    assert_eq!(
        block.lines().filter(|l| l.starts_with("- ")).count(),
        CHILD_STATUS_ROWS
    );
    assert!(block.contains("…and 12 more"), "{block}");
    assert!(
        block.len() < 1_200,
        "the block must stay small: {} bytes",
        block.len()
    );
}

#[test]
fn a_failed_read_says_so_instead_of_vanishing() {
    let block = render_child_status(Err(()));
    assert!(block.starts_with("[CHILD THREADS]"), "{block}");
    assert!(block.contains("Unreadable this turn"), "{block}");
}

#[test]
fn the_pointer_names_the_block_and_ranks_it_over_history() {
    assert!(CHILD_STATUS_POINTER.contains("[CHILD THREADS] block at the END"));
    assert!(CHILD_STATUS_POINTER.contains("beats any earlier report"));
}

mod db {
    use super::super::*;
    use crate::engine::event_bus::EventBus;
    use crate::engine::thread_events::{ChildCompletionStatus, ThreadEvent};
    use crate::test_support::{
        seed_chat_thread, seed_coding_thread, seed_thread_event, setup_test_db, teardown_test_db,
    };

    fn is_uuid_like(word: &str) -> bool {
        word.split(|c: char| !c.is_ascii_hexdigit() && c != '-')
            .any(|w| Uuid::parse_str(w).is_ok())
    }

    #[tokio::test]
    async fn the_block_reads_each_childs_liveness_now() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = seed_chat_thread(&bus, "plan the split").await;
        assert_eq!(child_status_block(&pool, parent).await, "");

        let child = seed_coding_thread(&bus, Some(parent)).await;
        seed_thread_event(
            &bus,
            parent,
            ThreadEvent::ChildThreadCompleted {
                child_thread_id: child,
                child_thread_title: Some("Split".into()),
                status: ChildCompletionStatus::Success,
                summary: "Waiting for the main repair.".into(),
                pending_change_ids: vec![],
                sub_thread_pending_changes: vec![],
                integrated_branch: None,
            },
        )
        .await;

        let block = child_status_block(&pool, parent).await;
        let expected = liveness_for_threads(&pool, &[child])
            .await
            .unwrap()
            .remove(&child)
            .expect("a summary row")
            .state
            .as_str()
            .replace('_', " ");
        assert!(block.contains(&format!("{expected} now")), "{block}");
        assert!(block.contains("last reported to you just now"), "{block}");
        assert!(
            !block.split_whitespace().any(is_uuid_like),
            "the block must name no id:\n{block}"
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_filed_report_counts_as_the_newest_report() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = seed_chat_thread(&bus, "parent").await;
        let child = seed_coding_thread(&bus, Some(parent)).await;
        seed_thread_event(
            &bus,
            parent,
            ThreadEvent::ChildReportFiled {
                child_thread_id: child,
                child_thread_title: Some("Split".into()),
                status: ChildCompletionStatus::Success,
                summary: "Filed without waking the parent.".into(),
                pending_change_ids: vec![],
                sub_thread_pending_changes: vec![],
                integrated_branch: None,
            },
        )
        .await;
        let block = child_status_block(&pool, parent).await;
        assert!(block.contains("last reported to you just now"), "{block}");
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_archived_child_drops_out() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = seed_chat_thread(&bus, "parent").await;
        let child = seed_coding_thread(&bus, Some(parent)).await;
        sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
            .bind(child)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(child_status_block(&pool, parent).await, "");
        teardown_test_db(&db).await;
    }
}
