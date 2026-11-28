use super::*;
use crate::core::changes::ChangeStatus;
use crate::engine::thread_events::AutomationUnpauseCause;
use crate::test_support::{
    boot_mock_engine, seed_idle_coding_thread, seed_thread_event, setup_test_db, teardown_test_db,
};

fn device() -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: "test-device".into(),
    })
}

async fn cards_raised(pool: &PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 \
         AND event_type IN ('UserQuestionAsked', 'UserQuestionQueued')",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn propose(bus: &EventBus, thread_id: Uuid) -> Uuid {
    let change_id = Uuid::new_v4();
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: change_id.to_string(),
            description: Some("held work".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: format!("claude-code/{change_id}"),
            repo_root: "/r".into(),
            hardened: true,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;
    change_id
}

async fn set_aside_by_no_hold(bus: &EventBus, thread_id: Uuid, change_id: Uuid) {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::ChangeSetAside {
            change_id: change_id.to_string(),
            hold_id: None,
        },
    )
    .await;
}

async fn status(engine: &LucidosEngine, change_id: Uuid) -> ChangeStatus {
    engine
        .changes()
        .get_by_id(change_id)
        .await
        .unwrap()
        .expect("change row")
        .status()
}

#[tokio::test]
async fn the_hold_folds_from_events_and_reads_a_stored_pause() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    assert!(thread_hold(&pool, thread).await.unwrap().is_none());
    assert!(may_start_turn(&pool, thread).await);

    let paused = seed_thread_event(
        &bus,
        thread,
        ThreadEvent::AutomationPaused {
            reason: "checkpoint".into(),
        },
    )
    .await;
    let hold = thread_hold(&pool, thread).await.unwrap().expect("held");
    assert_eq!(hold.hold_id, paused);
    assert_eq!(hold.root_thread_id, thread);
    assert_eq!(hold.reason, "checkpoint");
    assert!(!may_start_turn(&pool, thread).await);

    seed_thread_event(
        &bus,
        thread,
        ThreadEvent::AutomationUnpaused {
            cause: AutomationUnpauseCause::Unpause,
        },
    )
    .await;
    assert!(thread_hold(&pool, thread).await.unwrap().is_none());

    let hold_id = crate::test_support::seed_hold(&bus, thread).await;
    let (restarted, _rx) = EventBus::new(pool.clone());
    let hold = thread_hold(restarted.pool(), thread)
        .await
        .unwrap()
        .expect("held after a restart");
    assert_eq!(hold.hold_id, hold_id);
    assert!(!may_start_turn(&pool, thread).await);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn refusals_and_repeats_record_nothing() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread = seed_idle_coding_thread(&engine.event_bus, None).await;
    assert!(matches!(
        place_hold(&engine, thread, "  ", None, device()).await,
        Err(HoldError::Refused(_))
    ));
    assert!(matches!(
        place_hold(&engine, Uuid::new_v4(), "park", None, device()).await,
        Err(HoldError::NotFound)
    ));
    assert_eq!(
        place_hold(&engine, thread, " park it ", None, device())
            .await
            .unwrap(),
        HoldChange::Recorded
    );
    assert_eq!(
        place_hold(&engine, thread, "again", None, device())
            .await
            .unwrap(),
        HoldChange::Unchanged
    );
    let hold = thread_hold(&pool, thread).await.unwrap().unwrap();
    assert_eq!(hold.reason, "park it");
    assert_eq!(hold.placed_by, device());
    assert!(matches!(
        resume_hold(&engine, Uuid::new_v4(), device()).await,
        Err(HoldError::NotFound)
    ));
    let free = seed_idle_coding_thread(&engine.event_bus, None).await;
    assert_eq!(
        resume_hold(&engine, free, device()).await.unwrap(),
        HoldChange::Unchanged
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_repeat_finishes_a_partial_tree_placement() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let root = seed_idle_coding_thread(bus, None).await;
    let child = seed_idle_coding_thread(bus, Some(root)).await;
    let partial = crate::test_support::seed_hold(bus, root).await;
    assert!(thread_hold(&pool, child).await.unwrap().is_none());

    assert_eq!(
        place_hold(&engine, root, "again", None, device())
            .await
            .unwrap(),
        HoldChange::Recorded
    );
    let hold = thread_hold(&pool, child).await.unwrap().expect("finished");
    assert_eq!(hold.hold_id, partial, "the repeat joins the first hold");
    assert_eq!(
        place_hold(&engine, root, "again", None, device())
            .await
            .unwrap(),
        HoldChange::Unchanged
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn hold_reaches_the_tree_and_resume_lifts_exactly_what_it_placed() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let root = seed_idle_coding_thread(bus, None).await;
    let child = seed_idle_coding_thread(bus, Some(root)).await;
    let grandchild = seed_idle_coding_thread(bus, Some(child)).await;
    let held_before = seed_idle_coding_thread(bus, Some(root)).await;
    let earlier = crate::test_support::seed_hold(bus, held_before).await;

    place_hold(&engine, root, "park the tree", None, device())
        .await
        .unwrap();
    for thread in [root, child, grandchild] {
        let hold = thread_hold(&pool, thread).await.unwrap().expect("reached");
        assert_eq!(hold.root_thread_id, root, "{thread}");
        assert!(!may_start_turn(&pool, thread).await);
        assert_eq!(
            cards_raised(&pool, thread).await,
            0,
            "a hold raises no card"
        );
    }
    assert_eq!(
        thread_hold(&pool, held_before)
            .await
            .unwrap()
            .unwrap()
            .hold_id,
        earlier,
        "an earlier hold keeps its own id"
    );
    assert!(matches!(
        resume_hold(&engine, child, device()).await,
        Err(HoldError::Refused(_))
    ));

    let mut message = crate::test_support::message_received("still there?");
    if let ThreadEvent::MessageReceived { origin, .. } = &mut message {
        *origin = device();
    }
    seed_thread_event(bus, root, message).await;
    assert!(
        thread_hold(&pool, root).await.unwrap().is_some(),
        "a message does not resume"
    );

    resume_hold(&engine, root, device()).await.unwrap();
    for thread in [root, child, grandchild] {
        assert!(
            thread_hold(&pool, thread).await.unwrap().is_none(),
            "{thread}"
        );
    }
    assert_eq!(
        thread_hold(&pool, held_before)
            .await
            .unwrap()
            .unwrap()
            .hold_id,
        earlier,
        "Resume leaves an earlier hold"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn resume_brings_back_only_the_changes_its_hold_set_aside() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let root = seed_idle_coding_thread(bus, None).await;
    let child = seed_idle_coding_thread(bus, Some(root)).await;
    let older = propose(bus, root).await;
    set_aside_by_no_hold(bus, root, older).await;
    let pending_root = propose(bus, root).await;
    let pending_child = propose(bus, child).await;

    place_hold(&engine, root, "park", None, device())
        .await
        .unwrap();
    for change in [older, pending_root, pending_child] {
        assert_eq!(status(&engine, change).await, ChangeStatus::SetAside);
    }

    resume_hold(&engine, root, device()).await.unwrap();
    assert_eq!(status(&engine, pending_root).await, ChangeStatus::Pending);
    assert_eq!(status(&engine, pending_child).await, ChangeStatus::Pending);
    assert_eq!(
        status(&engine, older).await,
        ChangeStatus::SetAside,
        "a change set aside before the Hold stays set aside"
    );
    drop(pool);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_archive_lift_brings_nothing_back() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let thread = seed_idle_coding_thread(bus, None).await;
    let change = propose(bus, thread).await;
    place_hold(&engine, thread, "park", None, device())
        .await
        .unwrap();
    assert_eq!(
        lift_hold_on_archive(bus, thread, device()).await.unwrap(),
        HoldChange::Recorded
    );
    assert!(thread_hold(&pool, thread).await.unwrap().is_none());
    assert_eq!(status(&engine, change).await, ChangeStatus::SetAside);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_upgrade_holds_a_live_thread_with_a_set_aside_change() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let live = seed_idle_coding_thread(bus, None).await;
    let archived = seed_idle_coding_thread(bus, None).await;
    let live_change = propose(bus, live).await;
    let archived_change = propose(bus, archived).await;
    set_aside_by_no_hold(bus, live, live_change).await;
    set_aside_by_no_hold(bus, archived, archived_change).await;
    seed_thread_event(bus, archived, ThreadEvent::ThreadArchived).await;

    hold_set_aside_changes(bus, None).await;
    let hold = thread_hold(&pool, live)
        .await
        .unwrap()
        .expect("live thread held");
    assert_eq!(hold.reason, SET_ASIDE_HOLD_REASON);
    assert!(thread_hold(&pool, archived).await.unwrap().is_none());

    let events_before: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    hold_set_aside_changes(bus, None).await;
    let events_after: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(events_before, events_after, "a second boot records nothing");

    resume_hold(&engine, live, device()).await.unwrap();
    assert_eq!(status(&engine, live_change).await, ChangeStatus::Pending);
    assert_eq!(
        status(&engine, archived_change).await,
        ChangeStatus::SetAside
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_set_aside_change_holds_its_thread() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let thread = seed_idle_coding_thread(bus, None).await;
    let change = propose(bus, thread).await;
    engine
        .set_aside_change(change, None, device())
        .await
        .unwrap();
    let hold = thread_hold(&pool, thread)
        .await
        .unwrap()
        .expect("thread held");
    assert_eq!(hold.root_thread_id, thread);
    resume_hold(&engine, thread, device()).await.unwrap();
    assert_eq!(status(&engine, change).await, ChangeStatus::Pending);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_chat_thread_holds_too() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let chat = crate::test_support::seed_chat_thread(&engine.event_bus, "hello").await;
    place_hold(&engine, chat, "park", None, device())
        .await
        .unwrap();
    assert!(!may_start_turn(&pool, chat).await);
    teardown_test_db(&db).await;
}

async fn summary_hold(
    pool: &PgPool,
    thread_id: Uuid,
) -> Option<crate::core::store::thread_hold::ThreadHoldSummary> {
    crate::core::store::fetch_thread_aggregate(pool, thread_id)
        .await
        .unwrap()
        .expect("summary row")
        .hold
}

#[tokio::test]
async fn the_summary_hold_follows_hold_resume_and_a_stored_pause() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let root = seed_idle_coding_thread(bus, None).await;
    let child = seed_idle_coding_thread(bus, Some(root)).await;
    assert_eq!(summary_hold(&pool, root).await, None);

    place_hold(&engine, root, "park the tree", None, device())
        .await
        .unwrap();
    for thread in [root, child] {
        let hold = summary_hold(&pool, thread).await.expect("held summary");
        assert_eq!(hold.reason, "park the tree");
        assert_eq!(hold.root_thread_id, root.to_string());
        assert_eq!(hold.placed_by, device());
    }

    resume_hold(&engine, root, device()).await.unwrap();
    for thread in [root, child] {
        assert_eq!(summary_hold(&pool, thread).await, None, "{thread}");
    }

    seed_thread_event(
        bus,
        root,
        ThreadEvent::AutomationPaused {
            reason: "checkpoint".into(),
        },
    )
    .await;
    let hold = summary_hold(&pool, root).await.expect("a stored pause");
    assert_eq!(hold.root_thread_id, root.to_string());
    assert_eq!(hold.reason, "checkpoint");
    seed_thread_event(
        bus,
        root,
        ThreadEvent::AutomationUnpaused {
            cause: AutomationUnpauseCause::Unpause,
        },
    )
    .await;
    assert_eq!(summary_hold(&pool, root).await, None);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_backfill_matches_the_projection() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let held = seed_idle_coding_thread(bus, None).await;
    let lifted = seed_idle_coding_thread(bus, None).await;
    place_hold(&engine, held, "park", None, device())
        .await
        .unwrap();
    place_hold(&engine, lifted, "park", None, device())
        .await
        .unwrap();
    resume_hold(&engine, lifted, device()).await.unwrap();
    let projected = summary_hold(&pool, held).await.expect("held");

    sqlx::query("UPDATE thread_summaries SET hold = NULL")
        .execute(&pool)
        .await
        .unwrap();
    let migration =
        include_str!("../../../lucidos-core/migrations/20261123190407_add_thread_summary_hold.sql");
    sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    assert_eq!(summary_hold(&pool, held).await, Some(projected));
    assert_eq!(summary_hold(&pool, lifted).await, None);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn unarchive_brings_back_the_kept_work() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let thread = seed_idle_coding_thread(bus, None).await;
    let change = propose(bus, thread).await;
    seed_thread_event(bus, thread, ThreadEvent::ThreadArchived).await;
    set_aside_by_no_hold(bus, thread, change).await;
    seed_thread_event(bus, thread, ThreadEvent::ThreadUnarchived).await;
    bring_back_on_unarchive(&engine, thread, device()).await;
    assert_eq!(status(&engine, change).await, ChangeStatus::Pending);
    teardown_test_db(&db).await;
}
