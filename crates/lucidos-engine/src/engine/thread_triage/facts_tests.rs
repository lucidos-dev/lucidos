//! The facts loader against a real projection: which rows are roots, and what
//! rolls up into them.

use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

async fn spawn(bus: &EventBus, parent: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: parent,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    id
}

/// Settle a seeded row the way a finished turn leaves it.
async fn settle(pool: &PgPool, id: Uuid, set: &str) {
    sqlx::query(&format!(
        "UPDATE thread_summaries SET status = 'idle', has_response = TRUE, {set} \
         WHERE thread_id = $1"
    ))
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

async fn ask(bus: &EventBus, id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: format!("q-{id}"),
            cc_session_id: String::new(),
            question: "Which one?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

fn find(rows: &[TriageRow], id: Uuid) -> &TriageRow {
    rows.iter()
        .find(|r| r.facts.thread_id == id)
        .unwrap_or_else(|| panic!("{id} should be a root"))
}

#[tokio::test]
async fn the_loader_reads_inbox_roots_and_rolls_sub_threads_up() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let caller = spawn(&bus, None).await;
    let plain = spawn(&bus, None).await;
    settle(&pool, plain, "message_count = 2").await;
    let parent = spawn(&bus, None).await;
    let child = spawn(&bus, Some(parent)).await;
    settle(&pool, parent, "message_count = 2").await;
    settle(&pool, child, "compose_text = 'half a thought'").await;
    let archived = spawn(&bus, None).await;
    settle(&pool, archived, "archive_state = 'archived'").await;
    // An orphaned card: the turn ended with the question still unanswered.
    let orphan = spawn(&bus, None).await;
    ask(&bus, orphan).await;
    settle(&pool, orphan, "archive_state = 'inbox'").await;

    let rows = load(
        &pool,
        TriageScope::Inbox {
            except: Some(caller),
        },
    )
    .await
    .unwrap();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.facts.thread_id).collect();
    assert!(!ids.contains(&caller), "the caller is never triaged");
    assert!(
        !ids.contains(&child),
        "a sub-thread rolls up, it is no root"
    );
    assert!(
        !ids.contains(&archived),
        "an archived thread is not in the inbox"
    );

    let plain_row = find(&rows, plain);
    assert!(!plain_row.facts.needs_user());
    assert_eq!(plain_row.section, "inbox");

    let parent_row = find(&rows, parent);
    assert_eq!(parent_row.facts.sub_thread_count, 1);
    assert_eq!(parent_row.facts.sub_thread_needs, vec![NeedFact::Draft]);
    assert!(!parent_row.facts.has_draft, "the draft is the child's");

    let orphan_row = find(&rows, orphan);
    assert!(
        orphan_row.facts.has_pending_question,
        "an orphaned card counts as a pending question"
    );

    // The apply-time scope reads a named id wherever it sits now.
    let rows = load(&pool, TriageScope::Ids(&[archived])).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].section, "archived");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn only_a_triggers_newest_root_run_is_its_newest() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let older = spawn(&bus, None).await;
    let newer = spawn(&bus, None).await;
    for (id, age) in [(older, "2 days"), (newer, "1 day")] {
        settle(
            &pool,
            id,
            &format!(
                "source = 'trigger', trigger_id = 'watch', trigger_name = 'Watch', \
                 created_at = now() - interval '{age}'"
            ),
        )
        .await;
    }

    let rows = load(&pool, TriageScope::Inbox { except: None })
        .await
        .unwrap();
    let run = |id| {
        find(&rows, id)
            .facts
            .trigger
            .clone()
            .expect("a trigger run")
    };
    assert!(!run(older).is_newest);
    assert!(run(newer).is_newest);
    assert_eq!(run(newer).trigger_name, "Watch");

    teardown_test_db(&db).await;
}

/// Archiving a root cancels a sub-thread's event wait, and Undo cannot bring
/// the wait back. So a waiting sub-thread keeps its root, like a running one.
#[tokio::test]
async fn a_sub_threads_live_event_wait_keeps_its_root_busy() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None).await;
    let child = spawn(&bus, Some(parent)).await;
    settle(&pool, parent, "message_count = 2").await;
    settle(&pool, child, "live_event_wait_count = 1").await;

    let rows = load(&pool, TriageScope::Ids(&[parent])).await.unwrap();
    assert!(rows[0].facts.sub_thread_busy);
    assert!(rows[0].facts.busy_reason().is_some());

    teardown_test_db(&db).await;
}

/// The drawer hides a thread still being composed until it holds a draft, so
/// triage and Archive all never count one the user cannot see.
#[tokio::test]
async fn an_empty_composing_thread_is_not_a_root() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let empty = spawn(&bus, None).await;
    settle(&pool, empty, "state = 'composing'").await;
    let drafted = spawn(&bus, None).await;
    settle(
        &pool,
        drafted,
        "state = 'composing', compose_text = 'half a thought'",
    )
    .await;

    let rows = load(&pool, TriageScope::Inbox { except: None })
        .await
        .unwrap();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.facts.thread_id).collect();
    assert!(!ids.contains(&empty));
    assert!(find(&rows, drafted).facts.has_draft);

    teardown_test_db(&db).await;
}

/// Archive all counts inbox threads the way the drawer's badge does. An
/// archived sub-thread is not one. An inbox thread below it is its own root,
/// so it is counted there and not again under the root above.
#[tokio::test]
async fn a_root_counts_its_inbox_sub_threads_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let root = spawn(&bus, None).await;
    let open = spawn(&bus, Some(root)).await;
    let pinned = spawn(&bus, Some(root)).await;
    let archived = spawn(&bus, Some(root)).await;
    let below_archived = spawn(&bus, Some(archived)).await;
    for id in [root, open, below_archived] {
        settle(&pool, id, "archive_state = 'inbox'").await;
    }
    settle(&pool, pinned, "is_saved = TRUE").await;
    settle(&pool, archived, "archive_state = 'archived'").await;

    let rows = load(&pool, TriageScope::Inbox { except: None })
        .await
        .unwrap();
    let root_row = find(&rows, root);
    assert_eq!(
        root_row.inbox_sub_threads, 2,
        "the open and the pinned child"
    );
    assert_eq!(root_row.pinned_sub_threads, 1);
    assert_eq!(find(&rows, below_archived).inbox_sub_threads, 0);

    teardown_test_db(&db).await;
}

/// The drawer's family root is the topmost ancestor, through archived ones. A
/// triage root under a pinned family is pinned, so nothing archives a thread
/// the drawer shows under Pinned.
#[tokio::test]
async fn a_root_knows_its_drawer_family_and_whether_it_is_pinned() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let pinned_top = spawn(&bus, None).await;
    let archived_child = spawn(&bus, Some(pinned_top)).await;
    let under_pinned = spawn(&bus, Some(archived_child)).await;
    let archived_top = spawn(&bus, None).await;
    let pinned_child = spawn(&bus, Some(archived_top)).await;
    let plain_child = spawn(&bus, Some(archived_top)).await;
    settle(&pool, pinned_top, "is_saved = TRUE").await;
    for id in [archived_child, archived_top] {
        settle(&pool, id, "archive_state = 'archived'").await;
    }
    settle(&pool, pinned_child, "is_saved = TRUE").await;
    for id in [under_pinned, plain_child] {
        settle(&pool, id, "archive_state = 'inbox'").await;
    }

    let rows = load(&pool, TriageScope::Inbox { except: None })
        .await
        .unwrap();
    let row = find(&rows, under_pinned);
    assert_eq!(row.family_root, pinned_top);
    assert!(row.family_pinned);
    assert!(row.facts.is_pinned, "a pinned family pins its members");

    let pinned = find(&rows, pinned_child);
    assert_eq!(pinned.family_root, archived_top);
    assert!(!pinned.family_pinned);
    assert!(pinned.facts.is_pinned);
    let plain = find(&rows, plain_child);
    assert_eq!(plain.family_root, archived_top);
    assert!(!plain.facts.is_pinned);
    assert_eq!(find(&rows, pinned_top).family_root, pinned_top);

    teardown_test_db(&db).await;
}

/// The home thread is never a triage root, so neither triage nor Archive all
/// offers to put it away. Its sub-threads stand as roots of their own, the way
/// the drawer shows them (ADR 0362).
#[tokio::test]
async fn the_home_thread_is_not_a_root_and_its_sub_threads_are() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let child = spawn(&bus, Some(home)).await;
    settle(&pool, child, "archive_state = 'inbox'").await;

    let rows = load(&pool, TriageScope::Inbox { except: None })
        .await
        .unwrap();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.facts.thread_id).collect();
    assert!(!ids.contains(&home));
    assert!(ids.contains(&child));

    teardown_test_db(&db).await;
}
