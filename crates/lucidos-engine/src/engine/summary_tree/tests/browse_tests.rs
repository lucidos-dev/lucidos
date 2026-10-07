//! What a summary tree browser reads, against a real Postgres: the top of a
//! tree, the zoom it opens lines with, and the list of threads with trees.

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::seed;
use crate::engine::event_bus::EventBus;
use crate::engine::summary_tree::recall::{self, RecallLine};
use crate::engine::summary_tree::shape::complete_addresses;
use crate::engine::summary_tree::store::{self, StoredNode};
use crate::engine::summary_tree::view::NodeId;
use crate::engine::summary_tree::{NodeAddr, SummaryScope};
use crate::engine::thread_events::EventChannel;
use crate::test_support::{setup_test_db, teardown_test_db};

fn addr(start: u64, span: u64) -> NodeAddr {
    NodeAddr { start, span }
}

/// Every node of a complete workspace tree over `len` leaves, but `skip`.
async fn build_workspace_except(pool: &PgPool, len: u64, skip: &[NodeAddr]) {
    for a in complete_addresses(len) {
        if skip.contains(&a) {
            continue;
        }
        store::insert_node(
            pool,
            SummaryScope::Workspace,
            a,
            &StoredNode {
                text: format!("line {a}"),
                model: None,
                source_event_id: a.is_leaf().then(Uuid::new_v4),
                source_thread_id: None,
            },
        )
        .await
        .expect("insert node");
    }
}

fn ids(lines: &[RecallLine]) -> Vec<&str> {
    lines.iter().map(|l| l.id.as_str()).collect()
}

async fn zoom(pool: &PgPool, id: &str, levels: u32) -> Vec<RecallLine> {
    let id = NodeId::parse(id, None).unwrap();
    recall::zoom(pool, id, levels, &|_, _| None).await.unwrap()
}

/// The top tiles every built entry once, largest block first, and each merge
/// line is an id the recall tool's zoom opens. A leaf opens into its source,
/// which this fixture does not hold.
#[tokio::test]
async fn the_workspace_top_tiles_its_entries_with_ids_zoom_opens() {
    let (pool, db_name) = setup_test_db().await;
    build_workspace_except(&pool, 11, &[]).await;

    let top = recall::top(&pool, SummaryScope::Workspace).await.unwrap();
    assert_eq!(top.entries, 11);
    assert_eq!(ids(&top.lines), ["w/0+8", "w/8+2", "w/10+1"]);
    assert_eq!(top.lines[0].text, "line 0+8");
    for line in &top.lines {
        let id = NodeId::parse(&line.id, None).unwrap();
        if id.addr.is_leaf() {
            continue;
        }
        recall::zoom(&pool, id, 1, &|_, _| None)
            .await
            .unwrap_or_else(|e| panic!("zoom refused {}: {e}", line.id));
    }

    teardown_test_db(&db_name).await;
}

/// An unbuilt block shows its built halves, at the top and in a zoom alike.
#[tokio::test]
async fn an_unbuilt_block_shows_its_built_halves() {
    let (pool, db_name) = setup_test_db().await;
    build_workspace_except(&pool, 16, &[addr(0, 16), addr(0, 8)]).await;

    let top = recall::top(&pool, SummaryScope::Workspace).await.unwrap();
    assert_eq!(ids(&top.lines), ["w/0+4", "w/4+4", "w/8+8"]);
    assert_eq!(
        ids(&zoom(&pool, "w/0+16", 1).await),
        ["w/0+4", "w/4+4", "w/8+8"]
    );

    teardown_test_db(&db_name).await;
}

/// A leaf the compactor has not written yet becomes the pending line, after
/// the built ones, as the agent's zoom has always shown it.
#[tokio::test]
async fn a_missing_leaf_reads_as_pending() {
    let (pool, db_name) = setup_test_db().await;
    let gap = addr(5, 1);
    let unbuilt: Vec<NodeAddr> = complete_addresses(9)
        .into_iter()
        .filter(|a| a.start <= gap.start && gap.start < a.end())
        .collect();
    build_workspace_except(&pool, 9, &unbuilt).await;

    let lines = zoom(&pool, "w/0+8", 2).await;
    assert_eq!(
        ids(&lines),
        ["w/0+2", "w/2+2", "w/4+1", "w/6+2", "w/pending"]
    );
    assert!(
        lines[4].text.starts_with("1 newer entries"),
        "{}",
        lines[4].text
    );

    teardown_test_db(&db_name).await;
}

/// A thread's top reads every entry, from its event while no node is built.
#[tokio::test]
async fn a_thread_top_reads_its_unbuilt_entries_from_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    seed(
        &bus,
        thread,
        EventChannel::Chat,
        vec![
            json!({"type": "MessageReceived", "text": "the door is blue", "mode": "human"}),
            json!({"type": "ResponseGenerated", "text": "Noted: blue."}),
            json!({"type": "MessageReceived", "text": "and the gate?", "mode": "human"}),
        ],
    )
    .await;

    let top = recall::top(&pool, SummaryScope::Thread(thread))
        .await
        .unwrap();
    assert_eq!(top.entries, 3);
    assert_eq!(
        ids(&top.lines),
        [
            format!("{thread}/0+1"),
            format!("{thread}/1+1"),
            format!("{thread}/2+1"),
        ]
    );
    assert!(top.lines[0].text.contains("the door is blue"));

    teardown_test_db(&db_name).await;
}

/// The list holds in-scope threads only, newest activity first, each with
/// its count of summarised entries.
#[tokio::test]
async fn the_thread_list_skips_a_trigger_run_nobody_talked_to() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let older = Uuid::new_v4();
    let newer = Uuid::new_v4();
    let quiet = Uuid::new_v4();
    for thread in [older, newer] {
        seed(
            &bus,
            thread,
            EventChannel::Chat,
            vec![json!({"type": "MessageReceived", "text": "hello", "mode": "human"})],
        )
        .await;
    }
    seed(
        &bus,
        quiet,
        EventChannel::Trigger,
        vec![json!({"type": "TriggerStarted", "trigger_id": "t1", "prompt": "check the mail"})],
    )
    .await;
    sqlx::query(
        "UPDATE thread_summaries SET last_activity = now() - interval '1 day' WHERE thread_id = $1",
    )
    .bind(older)
    .execute(&pool)
    .await
    .unwrap();
    store::insert_node(
        &pool,
        SummaryScope::Thread(newer),
        NodeAddr::leaf(0),
        &StoredNode {
            text: "user: hello".to_string(),
            model: None,
            source_event_id: Some(Uuid::new_v4()),
            source_thread_id: None,
        },
    )
    .await
    .unwrap();

    let threads = store::tree_threads(&pool, 50, 0).await.unwrap();
    let listed: Vec<(Uuid, i64)> = threads
        .iter()
        .map(|t| (t.thread_id, t.summarised))
        .collect();
    assert_eq!(listed, [(newer, 1), (older, 0)]);
    assert_eq!(store::in_scope_thread_count(&pool).await.unwrap(), 2);
    let second_page = store::tree_threads(&pool, 1, 1).await.unwrap();
    assert_eq!(second_page[0].thread_id, older);

    teardown_test_db(&db_name).await;
}

/// A workspace line's date spans its leaves' source events, first to last,
/// read in one query rather than by shipping every leaf's id.
#[tokio::test]
async fn a_workspace_line_dates_its_leaves_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let events = seed(
        &bus,
        Uuid::new_v4(),
        EventChannel::Chat,
        vec![
            json!({"type": "MessageReceived", "text": "first", "mode": "human"}),
            json!({"type": "MessageReceived", "text": "second", "mode": "human"}),
        ],
    )
    .await;
    sqlx::query("UPDATE events SET created = now() - interval '2 days' WHERE id = $1")
        .bind(events[0])
        .execute(&pool)
        .await
        .unwrap();
    for (start, event) in events.iter().enumerate() {
        store::insert_node(
            &pool,
            SummaryScope::Workspace,
            NodeAddr::leaf(start as u64),
            &StoredNode {
                text: format!("leaf {start}"),
                model: None,
                source_event_id: Some(*event),
                source_thread_id: None,
            },
        )
        .await
        .unwrap();
    }

    let span = recall::date(&pool, NodeId::parse("w/0+2", None).unwrap())
        .await
        .unwrap();
    let from = chrono::DateTime::parse_from_rfc3339(span["from"].as_str().unwrap()).unwrap();
    let to = chrono::DateTime::parse_from_rfc3339(span["to"].as_str().unwrap()).unwrap();
    assert!(to - from > chrono::Duration::days(1), "{span}");

    teardown_test_db(&db_name).await;
}
